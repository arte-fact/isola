use std::path::PathBuf;

use crate::error::IsolaError;
use crate::paths;
use crate::sandbox::config::SandboxConfig;
use crate::sandbox::namespace::{SandboxExec, enter_sandbox};

use super::enter::build_env_vars;

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

    let ssh_dir = if config.share_ssh {
        std::env::var("HOME")
            .ok()
            .map(|h| std::path::PathBuf::from(h).join(".ssh"))
            .filter(|p| p.exists())
    } else {
        None
    };

    let exec_path = command[0].clone();
    let exec_args = command.clone();
    let env_vars = build_env_vars(true);

    let exec = SandboxExec {
        rootfs: rootfs.to_string_lossy().to_string(),
        exec_path,
        exec_args,
        env_vars,
        workspace_host: workspace.map(|p| p.to_string_lossy().to_string()),
        ssh_dir: ssh_dir.map(|p| p.to_string_lossy().to_string()),
        run_as_uid: Some(1000),
        multi_uid: true,
        capture_output: false,
    };

    enter_sandbox(exec)
}
