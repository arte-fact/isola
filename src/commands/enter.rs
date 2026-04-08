use std::path::PathBuf;

use crate::error::IsolaError;
use crate::sandbox::backend;
use crate::sandbox::config::SandboxConfig;

pub fn run(name: &str, shell: bool, workspace: Option<PathBuf>) -> Result<i32, IsolaError> {
    let config = SandboxConfig::load(name)?;
    let workspace = workspace.or(config.workspace);

    let b = backend::create_backend();
    b.enter_interactive(name, shell, workspace.as_deref())
}

/// Enter sandbox to run an arbitrary command as root (used by provisioning).
#[allow(dead_code)]
pub fn run_command(name: &str, command: &str) -> Result<i32, IsolaError> {
    let b = backend::create_backend();
    b.run_command(name, command)
}
