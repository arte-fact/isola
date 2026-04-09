use std::path::PathBuf;

use crate::error::IsolaError;
use crate::paths;
use crate::sandbox::config::SandboxConfig;
use crate::sandbox::namespace::{SandboxExec, enter_sandbox};

use super::enter::{build_env_vars, collect_host_mounts};

pub fn run(
    name: &str,
    command: Vec<String>,
    workspace: Option<PathBuf>,
) -> Result<i32, IsolaError> {
    let rootfs = paths::rootfs_dir(name);
    if !rootfs.exists() {
        return Err(IsolaError::SandboxNotFound(name.to_string()));
    }

    let config = SandboxConfig::load(name)?;
    let workspace = workspace.or(config.workspace);

    if command.is_empty() {
        return Err(IsolaError::ConfigError("no command specified".to_string()));
    }

    let host_mounts = collect_host_mounts(&config.environments);
    let exec_path = command[0].clone();
    let exec_args = command.clone();
    let mut env_vars = build_env_vars(true);
    env_vars.push(format!("ISOLA_SANDBOX={}", name));
    if config.share_display {
        for var in &["DISPLAY", "WAYLAND_DISPLAY", "XDG_SESSION_TYPE"] {
            if let Ok(val) = std::env::var(var) {
                env_vars.push(format!("{var}={val}"));
            }
        }
        env_vars.push("XAUTHORITY=/home/sandbox/.Xauthority".to_string());
    }

    let exec = SandboxExec {
        rootfs: rootfs.to_string_lossy().to_string(),
        exec_path,
        exec_args,
        env_vars,
        workspace_host: workspace.map(|p| p.to_string_lossy().to_string()),
        host_mounts,
        share_display: config.share_display,
        run_as_uid: Some(1000),
        multi_uid: true,
        capture_output: false,
    };

    enter_sandbox(exec)
}
