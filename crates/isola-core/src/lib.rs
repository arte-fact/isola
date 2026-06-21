//! # isola-core
//!
//! The engine behind [isola](https://github.com/arte-fact/isola): persistent,
//! isolated developer sandboxes built on Linux user namespaces (or a Lima VM on
//! macOS). This crate is the reusable library — it has no CLI or interactive
//! dependencies, so other applications (e.g. agent runners) can create, enter,
//! exec, and destroy sandboxes programmatically.
//!
//! Host setup (`host` module) is exposed as detection + AppArmor profile
//! generation; the library never prompts or calls `sudo` (that's the CLI's job).
//!
//! # Example
//!
//! ```no_run
//! use isola_core::{NoProgress, Sandbox, SandboxSpec};
//!
//! // Spin up a disposable, isolated environment for an agent task.
//! let spec = SandboxSpec {
//!     plugins: vec!["python-uv".into(), "git".into()],
//!     ..SandboxSpec::new("agent-42")
//! };
//! let sb = Sandbox::create(&spec, &NoProgress)?; // download + provision
//! let code = sb.exec(&["python".into(), "agent.py".into()], None, vec![])?;
//! sb.destroy()?; // throw it away when the task ends
//! # Ok::<(), isola_core::IsolaError>(())
//! ```

pub mod create;
pub mod error;
pub mod host;
pub mod paths;
pub mod plugin;
pub mod sandbox;

#[cfg(target_os = "linux")]
pub mod sha256;

pub use create::{CreateRequest, Sandbox, SandboxSpec};
pub use error::IsolaError;
pub use plugin::{Plugin, PluginRegistry};
pub use sandbox::config::{LocalConfig, SandboxConfig, SandboxShell};

/// Hook for reporting progress of sandbox creation. The CLI renders these live;
/// library consumers can pass [`NoProgress`] (or their own implementation) —
/// every method defaults to a no-op.
pub trait ProgressReporter {
    /// A step has started (shown as an in-progress line).
    fn start_step(&self, _msg: &str) {}
    /// A step has completed.
    fn finish_step(&self, _msg: &str) {}
    /// Rootfs download progress; `total` is 0 when the size is unknown.
    fn download(&self, _downloaded: u64, _total: u64) {}
    /// Full provisioning has begun.
    fn start_provision(&self) {}
    /// Provisioning advanced to phase `phase` of `total` (`name` describes it).
    fn provision_phase(&self, _phase: usize, _total: usize, _name: &str) {}
    /// A line of live provisioning output.
    fn provision_detail(&self, _line: &str) {}
    /// Creation finished by provisioning the given environments.
    fn finish_success(&self, _environments: &[String]) {}
    /// Creation finished by restoring a monolithic cache.
    fn finish_cached(&self, _environments: &[String]) {}
    /// Creation finished by assembling layers (some cached, some built).
    fn finish_layered(&self, _environments: &[String], _cached: &[String], _built: &[String]) {}
    /// Provisioning failed with the given exit code and tail of output.
    fn finish_error(&self, _exit_code: i32, _last_lines: &[String]) {}
}

/// A no-op [`ProgressReporter`] for non-interactive / library use.
pub struct NoProgress;

impl ProgressReporter for NoProgress {}
