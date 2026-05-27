use anyhow::{Context, Result};
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::time::ChronoLocal;
use tracing_subscriber::prelude::*;

const MAX_LINES_PER_FILE: u16 = 1000;

pub fn init_logger(exe_dir: &Path, debug_enabled: bool) -> Result<()> {
    let logs_dir = exe_dir.join("logs");
    fs::create_dir_all(&logs_dir)
        .with_context(|| format!("failed to create logs directory at {}", logs_dir.display()))?;

    let writer = Mutex::new(RotatingFileWriter::new(logs_dir));
    let timer = ChronoLocal::new("%Y-%m-%d %H:%M:%S%.3f".to_string());
    let level_filter = if debug_enabled {
        LevelFilter::DEBUG
    } else {
        LevelFilter::WARN
    };

    let layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_timer(timer)
        .with_target(false)
        .with_file(false)
        .with_line_number(false)
        .compact()
        .with_writer(writer)
        .with_filter(level_filter);

    tracing_subscriber::registry()
        .with(layer)
        .try_init()
        .context("failed to initialize logger")
}

struct RotatingFileWriter {
    logs_dir: PathBuf,
    line_count: u16,
    current: Option<BufWriter<File>>,
}

impl RotatingFileWriter {
    fn new(logs_dir: PathBuf) -> Self {
        Self {
            logs_dir,
            line_count: 0,
            current: None,
        }
    }

    fn ensure_writer(&mut self) -> io::Result<&mut BufWriter<File>> {
        if self.current.is_none() {
            let path = next_log_path(&self.logs_dir);
            let file = File::create(path)?;
            self.current = Some(BufWriter::new(file));
        }

        Ok(self.current.as_mut().expect("writer should be initialized"))
    }

    fn rotate_if_needed(&mut self) -> io::Result<()> {
        if self.line_count < MAX_LINES_PER_FILE {
            return Ok(());
        }

        if let Some(current) = self.current.as_mut() {
            current.flush()?;
        }

        let path = next_log_path(&self.logs_dir);
        let file = File::create(path)?;
        self.current = Some(BufWriter::new(file));
        self.line_count = 0;

        Ok(())
    }
}

impl Write for RotatingFileWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        for byte in buf {
            self.rotate_if_needed()?;
            self.ensure_writer()?
                .write_all(std::slice::from_ref(byte))?;
            if *byte == b'\n' {
                self.line_count = self.line_count.saturating_add(1);
            }
        }

        // Persist log output eagerly so diagnostics are visible while the tray daemon is still running.
        if let Some(current) = self.current.as_mut() {
            current.flush()?;
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(current) = self.current.as_mut() {
            current.flush()
        } else {
            Ok(())
        }
    }
}

fn next_log_path(logs_dir: &Path) -> PathBuf {
    let ts = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S_%3f");
    logs_dir.join(format!("process-display-helper-{ts}.log"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_logs_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after UNIX_EPOCH")
            .as_nanos();

        std::env::temp_dir().join(format!(
            "process-display-helper-logs-test-{}-{nanos}",
            std::process::id()
        ))
    }

    #[test]
    fn writer_does_not_create_file_until_first_write() {
        let logs_dir = unique_temp_logs_dir();
        fs::create_dir_all(&logs_dir).expect("failed to create test logs dir");

        let _writer = RotatingFileWriter::new(logs_dir.clone());
        let log_count = fs::read_dir(&logs_dir)
            .expect("failed to read test logs dir")
            .count();

        assert_eq!(log_count, 0);

        fs::remove_dir_all(&logs_dir).expect("failed to remove test logs dir");
    }

    #[test]
    fn writer_creates_file_on_first_write() {
        let logs_dir = unique_temp_logs_dir();
        fs::create_dir_all(&logs_dir).expect("failed to create test logs dir");

        let mut writer = RotatingFileWriter::new(logs_dir.clone());
        writer
            .write_all(b"hello world\n")
            .expect("failed writing test log line");
        writer.flush().expect("failed flushing test log writer");

        let mut entries = fs::read_dir(&logs_dir)
            .expect("failed to read test logs dir")
            .collect::<std::result::Result<Vec<_>, _>>()
            .expect("failed collecting test log entries");

        assert_eq!(entries.len(), 1);

        let entry = entries.pop().expect("expected one log file");
        let content = fs::read_to_string(entry.path()).expect("failed to read created log file");
        assert!(content.contains("hello world"));

        fs::remove_dir_all(&logs_dir).expect("failed to remove test logs dir");
    }
}
