use std::path::PathBuf;

use crate::error::IsolaError;
use crate::sandbox::backend;
use crate::sandbox::config::SandboxConfig;

pub fn run(name: &str, command: Vec<String>, workspace: Option<PathBuf>) -> Result<i32, IsolaError> {
    let config = SandboxConfig::load(name)?;
    let workspace = workspace.or(config.workspace);

    let b = backend::create_backend();
    b.exec_command(name, &command, workspace.as_deref())
}
