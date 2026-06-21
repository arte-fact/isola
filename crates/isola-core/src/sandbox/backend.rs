use std::path::Path;

use crate::error::IsolaError;
#[cfg(target_os = "macos")]
use crate::sandbox::rootfs;

pub trait SandboxBackend {
    /// Platform-specific pre-creation checks.
    fn preflight_checks(&self) -> Result<(), IsolaError>;

    /// Set up the sandbox environment (create Lima VM on macOS).
    /// macOS-only: the Linux path builds the rootfs directly in `create::run_linux`.
    #[cfg(target_os = "macos")]
    fn create_environment(&self, name: &str, workspace: Option<&Path>) -> Result<(), IsolaError>;

    /// Write configuration files into the sandbox (hostname, settings, CLAUDE.md, etc.).
    #[cfg(target_os = "macos")]
    fn write_sandbox_files(&self, name: &str, environments: &[String]) -> Result<(), IsolaError>;

    /// Enter the sandbox interactively (shell or Claude Code).
    fn enter_interactive(
        &self,
        name: &str,
        shell: bool,
        workspace: Option<&Path>,
        devices: Vec<String>,
    ) -> Result<i32, IsolaError>;

    /// Run a command inside the sandbox as root (used for provisioning).
    /// macOS-only: the Linux path uses `enter::run_command` directly.
    #[cfg(target_os = "macos")]
    fn run_command(&self, name: &str, command: &str) -> Result<i32, IsolaError>;

    /// Run an arbitrary command inside the sandbox as the sandbox user.
    fn exec_command(
        &self,
        name: &str,
        command: &[String],
        workspace: Option<&Path>,
        devices: Vec<String>,
    ) -> Result<i32, IsolaError>;

    /// Destroy the sandbox completely.
    fn destroy(&self, name: &str) -> Result<(), IsolaError>;

    /// Check if the sandbox is healthy/usable.
    fn is_healthy(&self, name: &str) -> bool;

    /// Backend name for display purposes.
    fn backend_name(&self) -> &'static str;

    /// URL or identifier for the base image used.
    #[cfg(target_os = "macos")]
    fn rootfs_url(&self) -> &'static str;

    /// Build a provisioning script for the given environments.
    /// Backends can override to add platform-specific steps (e.g. Claude CLI installation).
    #[cfg(target_os = "macos")]
    fn build_provision_script(
        &self,
        environments: &[String],
        plugin_vars: &std::collections::BTreeMap<String, String>,
    ) -> String {
        use crate::plugin::PluginRegistry;
        use crate::sandbox::config::SandboxShell;
        let registry = PluginRegistry::load().expect("failed to load plugin registry");
        rootfs::build_provision_script(
            environments,
            &SandboxShell::default(),
            &registry,
            plugin_vars,
        )
    }

    /// Description of the isolation mechanism (for CLAUDE.md).
    #[cfg(target_os = "macos")]
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
