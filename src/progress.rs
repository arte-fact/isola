use std::io::BufRead;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use console::style;

use crate::error::IsolaError;
use crate::sandbox::namespace::SandboxChild;

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
        let mut state = self.state.lock().unwrap();
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
        self.state.lock().unwrap().tick_idx += 1;
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
        let state = self.state.lock().unwrap();
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
            let mut state = self.inner.state.lock().unwrap();
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

    pub fn start_download(&self, total_bytes: u64) -> DownloadProgress {
        self.inner.stop_tick();
        {
            let mut state = self.inner.state.lock().unwrap();
            state.drawn_lines = 0;
        }
        DownloadProgress {
            total_bytes,
            inner: Arc::clone(&self.inner),
        }
    }

    pub fn finish_download(&self) {
        self.inner.stop_tick();
        eprintln!("  {} Downloaded rootfs", style("✓").green());
    }

    pub fn start_provision(&self) {
        self.inner.stop_tick();
        {
            let mut state = self.inner.state.lock().unwrap();
            state.spinner_msg = "Provisioning...".to_string();
            state.detail_msg.clear();
            state.drawn_lines = 0;
        }
        self.inner.start_tick();
    }

    pub fn set_provision_phase(&self, phase_num: usize, total: usize, name: &str) {
        let mut state = self.inner.state.lock().unwrap();
        state.spinner_msg = format!("Provisioning [{phase_num}/{total}] {name}...");
        state.detail_msg.clear();
    }

    pub fn set_provision_detail(&self, line: &str) {
        let truncated = truncate_to_width(line, 8);
        self.inner.state.lock().unwrap().detail_msg = truncated;
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

pub struct DownloadProgress {
    total_bytes: u64,
    inner: Arc<ProgressInner>,
}

impl DownloadProgress {
    pub fn set_position(&mut self, pos: u64) {
        let pct = if self.total_bytes > 0 {
            (pos as f64 / self.total_bytes as f64 * 100.0) as u64
        } else {
            0
        };
        let mb_done = pos as f64 / 1_048_576.0;
        let mb_total = self.total_bytes as f64 / 1_048_576.0;

        let msg = if self.total_bytes > 0 {
            let filled = (pct as usize * 20 / 100).min(20);
            let empty = 20 - filled;
            format!(
                "Downloading rootfs [{}{}] {:.1}/{:.1} MB ({pct}%)",
                "━".repeat(filled),
                " ".repeat(empty),
                mb_done,
                mb_total,
            )
        } else {
            format!("Downloading rootfs... {:.1} MB", mb_done)
        };

        {
            let mut state = self.inner.state.lock().unwrap();
            state.spinner_msg = msg;
        }
        self.inner.render();
    }
}

/// Count expected provisioning phases based on the `>>>` echo markers.
pub fn count_provision_phases(environments: &[String]) -> usize {
    let mut count = 3;
    for env in environments {
        match env.as_str() {
            "rust" | "nodejs" | "python-uv" | "go" => count += 1,
            _ => {}
        }
    }
    count += 2; // creating sandbox user + verifying
    count
}

/// Read piped provisioning output, update progress, return exit code.
pub fn monitor_provisioning(
    child: SandboxChild,
    progress: &CreationProgress,
    environments: &[String],
) -> Result<(i32, Vec<String>), IsolaError> {
    let total = count_provision_phases(environments);
    let mut current_phase = 0;
    let mut last_lines: Vec<String> = Vec::new();

    if let Some(output) = &child.output {
        let reader = std::io::BufReader::new(output);
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };

            if let Some(phase_name) = line.strip_prefix(">>> ") {
                current_phase += 1;
                let name = phase_name.trim_end_matches("...");
                progress.set_provision_phase(current_phase, total, name);
            } else if !line.is_empty() {
                progress.set_provision_detail(&line);
            }

            if !line.is_empty() {
                last_lines.push(line);
                if last_lines.len() > 10 {
                    last_lines.remove(0);
                }
            }
        }
    }

    let exit_code = child.wait()?;
    Ok((exit_code, last_lines))
}

fn truncate_to_width(line: &str, indent: usize) -> String {
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

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {}s", secs / 60, secs % 60)
    }
}
