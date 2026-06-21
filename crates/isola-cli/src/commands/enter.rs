use std::path::PathBuf;

use isola_core::error::IsolaError;
use isola_core::sandbox::backend;
use isola_core::sandbox::config::SandboxConfig;
use isola_core::sandbox::exec::collect_devices;

/// Enter a sandbox interactively (the `isola enter` command).
pub fn run(
    name: &str,
    workspace: Option<PathBuf>,
    cli_devices: Vec<String>,
) -> Result<i32, IsolaError> {
    let config = SandboxConfig::load(name)?;
    let workspace = workspace.or(config.workspace);

    let mut devices = collect_devices(&config.environments, &config.devices);
    for d in cli_devices {
        if !devices.contains(&d) {
            devices.push(d);
        }
    }

    let b = backend::create_backend();
    b.enter_interactive(name, false, workspace.as_deref(), devices)
}
