use anyhow::{Result, anyhow};
use ferrisetw::EventRecord;
use ferrisetw::native::EvntraceNativeError;
use ferrisetw::parser::Parser;
use ferrisetw::provider::Provider;
use ferrisetw::schema_locator::SchemaLocator;
use ferrisetw::trace::{TraceError, UserTrace, stop_trace_by_name};
use std::path::Path;
use std::sync::mpsc::Sender;

const PROCESS_PROVIDER_GUID: &str = "22fb2cd6-0e7b-422b-a0c7-2fad1fd0e716";
const PROCESS_START_EVENT_ID: u16 = 1;
const PROCESS_STOP_EVENT_ID: u16 = 2;
const TRACE_SESSION_NAME: &str = "process-display-helper";

#[derive(Debug, Clone)]
pub enum ProcessEvent {
    Started { name: String, pid: u32 },
    Exited { pid: u32 },
}

pub struct EtwHandle {
    trace: Option<UserTrace>,
}

impl EtwHandle {
    pub fn stop(mut self) {
        if let Some(trace) = self.trace.take() {
            if let Err(error) = trace.stop() {
                tracing::error!(error = ?error, "failed to stop ETW trace");

                if let Err(cleanup_error) = stop_trace_by_name(TRACE_SESSION_NAME) {
                    tracing::debug!(
                        trace_name = TRACE_SESSION_NAME,
                        error = ?cleanup_error,
                        "named ETW cleanup after stop failure was not needed or failed"
                    );
                }
            }
        }
    }
}

pub fn spawn_etw_listener(tx: Sender<ProcessEvent>) -> Result<EtwHandle> {
    let trace = match start_named_trace(&tx) {
        Ok(trace) => trace,
        Err(error) if is_already_exists_error(&error) => {
            tracing::warn!(
                trace_name = TRACE_SESSION_NAME,
                error = ?error,
                "stale ETW session found; stopping old session and retrying"
            );

            stop_trace_by_name(TRACE_SESSION_NAME).map_err(|cleanup_error| {
                anyhow!(
                    "failed to recover stale ETW trace session: startup_error={error:?}, cleanup_error={cleanup_error:?}"
                )
            })?;

            start_named_trace(&tx).map_err(|retry_error| {
                anyhow!("failed to start ETW process trace after cleanup: {retry_error:?}")
            })?
        }
        Err(error) => {
            return Err(anyhow!("failed to start ETW process trace: {error:?}"));
        }
    };

    Ok(EtwHandle { trace: Some(trace) })
}

fn start_named_trace(tx: &Sender<ProcessEvent>) -> std::result::Result<UserTrace, TraceError> {
    let callback_tx = tx.clone();
    let provider = Provider::by_guid(PROCESS_PROVIDER_GUID)
        .add_callback(
            move |record: &EventRecord, schema_locator: &SchemaLocator| {
                handle_process_event(record, schema_locator, &callback_tx);
            },
        )
        .build();

    UserTrace::new()
        .named(TRACE_SESSION_NAME.to_string())
        .enable(provider)
        .start_and_process()
}

fn is_already_exists_error(error: &TraceError) -> bool {
    matches!(
        error,
        TraceError::EtwNativeError(EvntraceNativeError::AlreadyExist)
    )
}

fn handle_process_event(
    record: &EventRecord,
    schema_locator: &SchemaLocator,
    tx: &Sender<ProcessEvent>,
) {
    let event_id = record.event_id();
    if event_id != PROCESS_START_EVENT_ID && event_id != PROCESS_STOP_EVENT_ID {
        return;
    }

    let schema = match schema_locator.event_schema(record) {
        Ok(schema) => schema,
        Err(error) => {
            tracing::debug!(error = ?error, "failed to resolve ETW schema");
            return;
        }
    };

    let parser = Parser::create(record, &schema);
    let pid: u32 = match parser.try_parse("ProcessID") {
        Ok(pid) => pid,
        Err(_) => return,
    };

    if event_id == PROCESS_START_EVENT_ID {
        let image_name: String = parser.try_parse("ImageName").unwrap_or_default();
        let name = normalize_process_name(&image_name);
        if name.is_empty() {
            return;
        }

        let _ = tx.send(ProcessEvent::Started { name, pid });
        return;
    }

    let _ = tx.send(ProcessEvent::Exited { pid });
}

fn normalize_process_name(name: &str) -> String {
    let candidate = Path::new(name)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(name);

    candidate.trim().to_ascii_lowercase()
}
