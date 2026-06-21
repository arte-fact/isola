//! # isola-core
//!
//! The engine behind [isola](https://github.com/arte-fact/isola): persistent,
//! isolated developer sandboxes built on Linux user namespaces (or a Lima VM on
//! macOS). This crate is the reusable library — it has no CLI or interactive
//! dependencies, so other applications (e.g. agent runners) can create, enter,
//! exec, and destroy sandboxes programmatically.
//!
//! Host setup (`host` module) is exposed as detection + an explicit, privileged,
//! non-interactive `prepare`; the library never prompts or calls `sudo`.

pub mod error;
pub mod host;
pub mod paths;
pub mod plugin;
pub mod sandbox;

#[cfg(target_os = "linux")]
pub mod sha256;

pub use error::IsolaError;
pub use plugin::{Plugin, PluginRegistry};
pub use sandbox::config::{LocalConfig, SandboxConfig, SandboxShell};

/// Hook for reporting progress of long operations (currently the one-time rootfs
/// download). The CLI renders these live; library consumers can pass
/// [`NoProgress`] (or their own implementation) — all methods default to no-ops.
pub trait ProgressReporter {
    /// A step has started (shown as an in-progress line).
    fn start_step(&self, _msg: &str) {}
    /// A step has completed.
    fn finish_step(&self, _msg: &str) {}
    /// Download progress; `total` is 0 when the size is unknown.
    fn download(&self, _downloaded: u64, _total: u64) {}
}

/// A no-op [`ProgressReporter`] for non-interactive / library use.
pub struct NoProgress;

impl ProgressReporter for NoProgress {}
