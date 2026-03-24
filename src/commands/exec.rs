use std::path::PathBuf;

use crate::error::BotError;
use crate::paths;
use crate::sandbox::config::SandboxConfig;
use crate::sandbox::namespace::{SandboxExec, enter_sandbox};

use super::enter::build_env_vars;

pub fn run(name: &str, command: Vec<String>, workspace: Option<PathBuf>) -> Result<i32, BotError> {
    let rootfs = paths::rootfs_dir(name);
    if !rootfs.exists() {
        return Err(BotError::SandboxNotFound(name.to_string()));
    }

    let config = SandboxConfig::load(name)?;
    let workspace = workspace.or(config.workspace);

    if command.is_empty() {
        return Err(BotError::ConfigError("no command specified".to_string()));
    }

    let exec_path = command[0].clone();
    let exec_args = command.clone();
    let env_vars = build_env_vars(true);

    let exec = SandboxExec {
        rootfs: rootfs.to_string_lossy().to_string(),
        exec_path,
        exec_args,
        env_vars,
        workspace_host: workspace.map(|p| p.to_string_lossy().to_string()),
        claude_binary: None,
        session_credentials: None,
        run_as_uid: Some(1000),
        multi_uid: true,
    };

    enter_sandbox(exec)
}
