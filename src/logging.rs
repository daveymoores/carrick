use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;
use tracing_appender::rolling;
use tracing_subscriber::{EnvFilter, Layer, fmt, layer::SubscriberExt, util::SubscriberInitExt};

/// Initialize the global tracing subscriber with two layers:
///
/// 1. **Terminal layer** (stderr): Shows `INFO` by default, `DEBUG` with `--verbose`.
///    Uses a minimal format without timestamps or targets for a clean look.
///
/// 2. **File layer**: Always writes `DEBUG`-level logs with timestamps to
///    `~/.carrick/logs/carrick.log` (daily rotation).
///
/// The file layer is best-effort — if the log directory can't be created, only
/// the terminal layer is active.
pub fn init(verbose: bool) {
    let terminal_filter = if verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::new("info")
    };

    let terminal_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(false)
        .with_level(false)
        .without_time()
        .with_filter(terminal_filter);

    // Try to set up file logging to ~/.carrick/logs/
    let log_dir = dirs::home_dir().map(|h| h.join(".carrick").join("logs"));

    if let Some(ref dir) = log_dir {
        if std::fs::create_dir_all(dir).is_ok() {
            let file_appender = rolling::daily(dir, "carrick.log");
            let file_layer = fmt::layer()
                .with_writer(file_appender)
                .with_ansi(false)
                .with_target(true)
                .with_filter(EnvFilter::new("debug"));

            tracing_subscriber::registry()
                .with(terminal_layer)
                .with(file_layer)
                .init();
            return;
        }
    }

    // Fallback: terminal only
    tracing_subscriber::registry().with(terminal_layer).init();
}

/// Return the path to today's log file, if it exists.
///
/// The rolling appender creates files named `carrick.log.YYYY-MM-DD`.
pub fn get_log_file_path() -> Option<std::path::PathBuf> {
    let log_dir = dirs::home_dir()?.join(".carrick").join("logs");
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let log_file = log_dir.join(format!("carrick.log.{}", today));
    if log_file.exists() {
        Some(log_file)
    } else {
        None
    }
}

/// Create a spinner with a message. Call `finish_with_message` when done.
pub fn spinner(msg: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner:.cyan} {msg}")
            .unwrap()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
    );
    pb.set_message(msg.to_string());
    pb.enable_steady_tick(Duration::from_millis(80));
    pb
}

/// Finish a spinner with a success checkmark.
pub fn finish_spinner(pb: &ProgressBar, msg: &str) {
    pb.set_style(ProgressStyle::with_template("  {msg}").unwrap());
    pb.finish_with_message(format!("\x1b[32m✓\x1b[0m {}", msg));
}

/// Finish a spinner with a warning marker.
pub fn finish_spinner_warn(pb: &ProgressBar, msg: &str) {
    pb.set_style(ProgressStyle::with_template("  {msg}").unwrap());
    pb.finish_with_message(format!("\x1b[33m⚠\x1b[0m {}", msg));
}
