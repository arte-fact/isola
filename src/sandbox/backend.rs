use std::path::Path;

use crate::error::IsolaError;
use crate::sandbox::rootfs;

pub trait SandboxBackend {
    /// Platform-specific pre-creation checks.
    fn preflight_checks(&self) -> Result<(), IsolaError>;

    /// Set up the sandbox environment (extract rootfs on Linux, create Lima VM on macOS).
    fn create_environment(&self, name: &str, workspace: Option<&Path>) -> Result<(), IsolaError>;

    /// Write configuration files into the sandbox (hostname, settings, CLAUDE.md, etc.).
    fn write_sandbox_files(&self, name: &str, environments: &[String]) -> Result<(), IsolaError>;

    /// Enter the sandbox interactively (shell or Claude Code).
    fn enter_interactive(
        &self,
        name: &str,
        shell: bool,
        workspace: Option<&Path>,
    ) -> Result<i32, IsolaError>;

    /// Run a command inside the sandbox as root (used for provisioning).
    fn run_command(&self, name: &str, command: &str) -> Result<i32, IsolaError>;

    /// Run an arbitrary command inside the sandbox as the sandbox user.
    fn exec_command(
        &self,
        name: &str,
        command: &[String],
        workspace: Option<&Path>,
    ) -> Result<i32, IsolaError>;

    /// Destroy the sandbox completely.
    fn destroy(&self, name: &str) -> Result<(), IsolaError>;

    /// Check if the sandbox is healthy/usable.
    fn is_healthy(&self, name: &str) -> bool;

    /// Backend name for display purposes.
    fn backend_name(&self) -> &'static str;

    /// URL or identifier for the base image used.
    fn rootfs_url(&self) -> &'static str;

    /// Build a provisioning script for the given environments.
    /// Backends can override to add platform-specific steps (e.g. Claude CLI installation).
    fn build_provision_script(&self, environments: &[String]) -> String {
        rootfs::build_provision_script(environments)
    }

    /// Description of the isolation mechanism (for CLAUDE.md).
    fn isolation_description(&self) -> &'static str;
}

#[cfg(target_os = "linux")]
pub fn create_backend() -> Box<dyn SandboxBackend> {
    Box::new(super::linux::LinuxBackend)
}

#[cfg(target_os = "macos")]
pub fn create_backend() -> Box<dyn SandboxBackend> {
    Box::new(super::lima::LimaBackend)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("isola only supports Linux and macOS");
