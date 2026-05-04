use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;
use tracing::info;
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

    if let Some(ref dir) = log_dir
        && std::fs::create_dir_all(dir).is_ok()
    {
        let file_appender = rolling::daily(dir, "carrick.log");
        let file_layer = fmt::layer()
            .with_writer(file_appender)
            .with_ansi(false)
            .with_target(true)
            .with_filter(EnvFilter::new("debug"));

        let _ = tracing_subscriber::registry()
            .with(terminal_layer)
            .with(file_layer)
            .try_init();
        emit_run_preamble();
        return;
    }

    // Fallback: terminal only
    let _ = tracing_subscriber::registry()
        .with(terminal_layer)
        .try_init();
    emit_run_preamble();
}

/// Emit a structured preamble at the start of every run. Goes through `tracing`
/// so it lands in both the rolling file log (always) and the terminal (info+).
/// This is the "what was the environment when this ran" record that makes
/// uploaded logs interpretable after the fact.
fn emit_run_preamble() {
    fn env(name: &str) -> String {
        std::env::var(name).unwrap_or_else(|_| "<unset>".to_string())
    }

    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();

    info!(
        scanner_version = env!("CARGO_PKG_VERSION"),
        api_endpoint = env!("CARRICK_API_ENDPOINT"),
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        ci = %env("CI"),
        github_event = %env("GITHUB_EVENT_NAME"),
        github_ref = %env("GITHUB_REF"),
        github_repo = %env("GITHUB_REPOSITORY"),
        github_sha = %env("GITHUB_SHA"),
        github_run_id = %env("GITHUB_RUN_ID"),
        github_workflow = %env("GITHUB_WORKFLOW"),
        runner_os = %env("RUNNER_OS"),
        cwd = %cwd,
        "Carrick run starting"
    );
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
