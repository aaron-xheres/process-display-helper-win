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

    let writer = Mutex::new(RotatingFileWriter::new(logs_dir)?);
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
    current: BufWriter<File>,
}

impl RotatingFileWriter {
    fn new(logs_dir: PathBuf) -> Result<Self> {
        let path = next_log_path(&logs_dir);
        let file = File::create(&path)
            .with_context(|| format!("failed to create log file at {}", path.display()))?;

        Ok(Self {
            logs_dir,
            line_count: 0,
            current: BufWriter::new(file),
        })
    }

    fn rotate_if_needed(&mut self) -> io::Result<()> {
        if self.line_count < MAX_LINES_PER_FILE {
            return Ok(());
        }

        self.current.flush()?;
        let path = next_log_path(&self.logs_dir);
        let file = File::create(path)?;
        self.current = BufWriter::new(file);
        self.line_count = 0;

        Ok(())
    }
}

impl Write for RotatingFileWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        for byte in buf {
            self.rotate_if_needed()?;
            self.current.write_all(std::slice::from_ref(byte))?;
            if *byte == b'\n' {
                self.line_count = self.line_count.saturating_add(1);
            }
        }

        // Persist log output eagerly so diagnostics are visible while the tray daemon is still running.
        self.current.flush()?;

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.current.flush()
    }
}

fn next_log_path(logs_dir: &Path) -> PathBuf {
    let ts = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S_%3f");
    logs_dir.join(format!("process-display-helper-{ts}.log"))
}
