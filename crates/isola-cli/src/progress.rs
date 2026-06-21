use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use console::style;

const TICK_STRINGS: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Progress UI for sandbox creation.
///
/// Manually renders 1-2 live lines at the bottom of stderr using ANSI escapes.
/// All rendering happens in a single atomic `stderr.lock()` + `write!` + `flush`
/// to avoid partial/interleaved output.
pub struct CreationProgress {
    inner: Arc<ProgressInner>,
}

struct ProgressInner {
    state: Mutex<ProgressState>,
    start_time: Instant,
    ticking: AtomicBool,
}

struct ProgressState {
    spinner_msg: String,
    detail_msg: String,
    tick_idx: usize,
    drawn_lines: usize,
}

impl ProgressInner {
    /// Single atomic operation: erase previous live lines, draw new ones.
    /// Holds stderr lock for the entire sequence so nothing can interleave.
    fn render(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut stderr = std::io::stderr().lock();
        let mut buf = String::with_capacity(256);

        // Erase: for each previously drawn line, move up + clear
        for _ in 0..state.drawn_lines {
            buf.push_str("\x1b[A\x1b[2K");
        }
        // Ensure cursor is at column 0
        buf.push('\r');

        // Draw line 1: spinner
        let tick = TICK_STRINGS[state.tick_idx % TICK_STRINGS.len()];
        buf.push_str(&format!(
            "  \x1b[36m{tick}\x1b[0m {}\x1b[K\n",
            state.spinner_msg
        ));
        state.drawn_lines = 1;

        // Draw line 2: detail (optional)
        if !state.detail_msg.is_empty() {
            buf.push_str(&format!("    \x1b[2m└ {}\x1b[0m\x1b[K\n", state.detail_msg));
            state.drawn_lines = 2;
        }

        let _ = write!(stderr, "{buf}");
        let _ = stderr.flush();
    }

    fn tick(&self) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .tick_idx += 1;
        self.render();
    }

    fn start_tick(self: &Arc<Self>) {
        if self.ticking.swap(true, Ordering::SeqCst) {
            return;
        }
        let inner = Arc::clone(self);
        std::thread::spawn(move || {
            while inner.ticking.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(80));
                if inner.ticking.load(Ordering::SeqCst) {
                    inner.tick();
                }
            }
        });
    }

    fn stop_tick(&self) {
        self.ticking.store(false, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(10));
        // Erase live lines
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.drawn_lines > 0 {
            let mut stderr = std::io::stderr().lock();
            let mut buf = String::new();
            for _ in 0..state.drawn_lines {
                buf.push_str("\x1b[A\x1b[2K");
            }
            buf.push('\r');
            let _ = write!(stderr, "{buf}");
            let _ = stderr.flush();
        }
    }
}

impl CreationProgress {
    pub fn new(sandbox_name: &str) -> Self {
        eprintln!("  {} Creating sandbox '{sandbox_name}'", style("●").cyan());
        Self {
            inner: Arc::new(ProgressInner {
                state: Mutex::new(ProgressState {
                    spinner_msg: String::new(),
                    detail_msg: String::new(),
                    tick_idx: 0,
                    drawn_lines: 0,
                }),
                start_time: Instant::now(),
                ticking: AtomicBool::new(false),
            }),
        }
    }

    pub fn start_step(&self, msg: &str) {
        self.inner.stop_tick();
        {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            state.spinner_msg = msg.to_string();
            state.detail_msg.clear();
            state.drawn_lines = 0;
        }
        self.inner.start_tick();
    }

    pub fn finish_step(&self, msg: &str) {
        self.inner.stop_tick();
        eprintln!("  {} {msg}", style("✓").green());
    }

    pub fn start_provision(&self) {
        self.inner.stop_tick();
        {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            state.spinner_msg = "Provisioning...".to_string();
            state.detail_msg.clear();
            state.drawn_lines = 0;
        }
        self.inner.start_tick();
    }

    pub fn set_provision_phase(&self, phase_num: usize, total: usize, name: &str) {
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        state.spinner_msg = format!("Provisioning [{phase_num}/{total}] {name}...");
        state.detail_msg.clear();
    }

    pub fn set_provision_detail(&self, line: &str) {
        let truncated = truncate_to_width(line, 8);
        self.inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .detail_msg = truncated;
    }

    pub fn finish_success(&self, environments: &[String]) {
        self.inner.stop_tick();
        let elapsed = self.inner.start_time.elapsed();
        let elapsed_str = format_duration(elapsed);
        eprintln!(
            "  {} Provisioned ({}) in {elapsed_str}",
            style("✓").green(),
            environments.join(", "),
        );
        eprintln!("\n  {}", style("✨ Sandbox is ready!").green().bold());
    }

    pub fn finish_cached(&self, environments: &[String]) {
        self.inner.stop_tick();
        let elapsed = self.inner.start_time.elapsed();
        let elapsed_str = format_duration(elapsed);
        eprintln!(
            "  {} Restored from cache ({}) in {elapsed_str}",
            style("✓").green(),
            environments.join(", "),
        );
        eprintln!("\n  {}", style("✨ Sandbox is ready!").green().bold());
    }

    pub fn finish_layered(
        &self,
        environments: &[String],
        cached_layers: &[String],
        built_layers: &[String],
    ) {
        self.inner.stop_tick();
        let elapsed = self.inner.start_time.elapsed();
        let elapsed_str = format_duration(elapsed);
        if built_layers.is_empty() {
            eprintln!(
                "  {} Assembled from cached layers ({}) in {elapsed_str}",
                style("✓").green(),
                environments.join(", "),
            );
        } else {
            eprintln!(
                "  {} Ready ({}) in {elapsed_str} — cached: [{}], built: [{}]",
                style("✓").green(),
                environments.join(", "),
                cached_layers.join(", "),
                built_layers.join(", "),
            );
        }
        eprintln!("\n  {}", style("✨ Sandbox is ready!").green().bold());
    }

    pub fn finish_error(&self, exit_code: i32, last_lines: &[String]) {
        self.inner.stop_tick();
        eprintln!(
            "  {} Provisioning failed (exit code {exit_code})",
            style("✗").red(),
        );
        if !last_lines.is_empty() {
            eprintln!("  {}", style("Last output:").dim());
            for line in last_lines {
                eprintln!("  {} {line}", style("│").dim());
            }
        }
    }
}
pub(crate) fn truncate_to_width(line: &str, indent: usize) -> String {
    let width = console::Term::stderr().size().1.max(40) as usize;
    let available = width.saturating_sub(indent);
    if line.len() > available {
        let mut end = available.saturating_sub(1);
        while end > 0 && !line.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &line[..end])
    } else {
        line.to_string()
    }
}

pub(crate) fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {}s", secs / 60, secs % 60)
    }
}

/// Let the core engine drive this progress UI via the `ProgressReporter` trait.
/// The one-time rootfs download is shown as a percentage on the spinner line.
impl isola_core::ProgressReporter for CreationProgress {
    fn start_step(&self, msg: &str) {
        CreationProgress::start_step(self, msg);
    }
    fn finish_step(&self, msg: &str) {
        CreationProgress::finish_step(self, msg);
    }
    fn download(&self, downloaded: u64, total: u64) {
        let msg = if total > 0 {
            format!(
                "Downloading base rootfs… {}%",
                downloaded.saturating_mul(100) / total
            )
        } else {
            format!("Downloading base rootfs… {} MiB", downloaded / 1_048_576)
        };
        CreationProgress::start_step(self, &msg);
    }
    fn start_provision(&self) {
        CreationProgress::start_provision(self);
    }
    fn provision_phase(&self, phase: usize, total: usize, name: &str) {
        CreationProgress::set_provision_phase(self, phase, total, name);
    }
    fn provision_detail(&self, line: &str) {
        CreationProgress::set_provision_detail(self, line);
    }
    fn finish_success(&self, environments: &[String]) {
        CreationProgress::finish_success(self, environments);
    }
    fn finish_cached(&self, environments: &[String]) {
        CreationProgress::finish_cached(self, environments);
    }
    fn finish_layered(&self, environments: &[String], cached: &[String], built: &[String]) {
        CreationProgress::finish_layered(self, environments, cached, built);
    }
    fn finish_error(&self, exit_code: i32, last_lines: &[String]) {
        CreationProgress::finish_error(self, exit_code, last_lines);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_duration_seconds() {
        assert_eq!(format_duration(Duration::from_secs(42)), "42s");
    }

    #[test]
    fn format_duration_zero() {
        assert_eq!(format_duration(Duration::from_secs(0)), "0s");
    }

    #[test]
    fn format_duration_exactly_60() {
        assert_eq!(format_duration(Duration::from_secs(60)), "1m 0s");
    }

    #[test]
    fn format_duration_minutes_and_seconds() {
        assert_eq!(format_duration(Duration::from_secs(125)), "2m 5s");
    }

    #[test]
    fn truncate_short_string_unchanged() {
        let result = truncate_to_width("hello", 8);
        assert_eq!(result, "hello");
    }

    #[test]
    fn truncate_handles_empty_string() {
        let result = truncate_to_width("", 8);
        assert_eq!(result, "");
    }
}
