use std::path::PathBuf;

use isola_core::error::IsolaError;
use isola_core::sandbox::backend;
use isola_core::sandbox::config::SandboxConfig;

use isola_core::sandbox::exec::collect_devices;

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
