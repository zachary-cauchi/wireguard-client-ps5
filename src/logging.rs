use std::fmt::Arguments;

use tracing::level_filters::LevelFilter;
use tracing_subscriber::{
    Layer, Registry,
    fmt::{self},
    layer::SubscriberExt,
};

/// Handles initialising program-wide logging.
pub struct Ps5LoggingBuilder {
    /// Klog level filter.
    k_level: LevelFilter,
    /// Stdout level filter.
    std_level: LevelFilter,
}

impl Ps5LoggingBuilder {
    pub fn new() -> Self {
        Self {
            k_level: LevelFilter::TRACE,
            std_level: LevelFilter::INFO,
        }
    }

    pub fn with_klog_level(self, level: impl Into<LevelFilter>) -> Self {
        Self {
            k_level: level.into(),
            ..self
        }
    }

    pub fn with_std_level(self, level: impl Into<LevelFilter>) -> Self {
        Self {
            std_level: level.into(),
            ..self
        }
    }

    /// Creates a new tracing subscriber which logs to both stdout and klog,
    /// then installs it globally.
    pub fn install_global(self) -> crate::Result<()> {
        let klog_layer = fmt::layer()
            .with_writer(|| KLogWriter {})
            .with_ansi(false)
            .with_filter(self.k_level);

        let std_layer = fmt::layer()
            .with_writer(|| StdWriter {})
            .with_filter(self.std_level);

        let final_layers = Registry::default().with(klog_layer).with(std_layer);

        tracing::subscriber::set_global_default(final_layers)?;

        Ok(())
    }
}

/// Thin writer for writing messages to the PS5 klog.
struct KLogWriter {}

impl std::io::Write for KLogWriter {
    /// Write to klog.
    /// Klog truncates msgs longer than 1,023 bytes so if [buf] is longer than 1,023 bytes, only the first 1,023 bytes are written.
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        const LEN_CUTOFF: usize = 1024;

        let slice_len = buf.len().min(LEN_CUTOFF);
        let line = str::from_utf8(&buf[0..slice_len]).map_err(|e| std::io::Error::other(e))?;

        ps5::klog::log(line);

        Ok(slice_len)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Thin writer for writing messages to Stdout.
struct StdWriter {}

impl std::io::Write for StdWriter {
    /// Truncate msgs to 1,023 bytes max to keep in line with klog limitation.
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        const LEN_CUTOFF: usize = 1024;

        let slice_len = buf.len().min(LEN_CUTOFF);

        ps5::fmt::stdout(&buf[0..slice_len]);

        Ok(slice_len)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub fn klog_log(args: Arguments<'_>) {
    if let Some(args_string) = args.as_str() {
        ps5::log!(args_string);
    } else {
        ps5::log!(args.to_string().as_str());
    }
}

pub fn stdout_log(args: Arguments<'_>) {
    if let Some(args_string) = args.as_str() {
        ps5::print!(args_string);
    } else {
        ps5::print!(args.to_string().as_str());
    }
}

#[macro_export]
macro_rules! klog_log {
    ($($arg:tt)*) => {
        $crate::logging::klog_log(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! stdout_log {
    ($($arg:tt)*) => {
        $crate::logging::stdout_log(format_args!($($arg)*))
    };
}
