use std::path::PathBuf;

use crate::error::IsolaError;
use crate::sandbox::backend;
use crate::sandbox::config::SandboxConfig;

use super::enter::collect_devices;

pub fn run(
    name: &str,
    command: Vec<String>,
    workspace: Option<PathBuf>,
    cli_devices: Vec<String>,
) -> Result<i32, IsolaError> {
    let config = SandboxConfig::load(name)?;
    let workspace = workspace.or(config.workspace);

    if command.is_empty() {
        return Err(IsolaError::ConfigError("no command specified".to_string()));
    }

    let mut devices = collect_devices(&config.environments, &config.devices);
    for d in cli_devices {
        if !devices.contains(&d) {
            devices.push(d);
        }
    }

    let b = backend::create_backend();
    b.exec_command(name, &command, workspace.as_deref(), devices)
}
