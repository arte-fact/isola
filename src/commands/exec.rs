use std::path::PathBuf;

use crate::error::IsolaError;
use crate::sandbox::config::SandboxConfig;

pub fn run(name: &str, command: Vec<String>, workspace: Option<PathBuf>) -> Result<i32, IsolaError> {
    let workspace = workspace.or_else(|| SandboxConfig::load(name).ok().and_then(|c| c.workspace));
    crate::backend::exec_command(name, command, workspace)
}
