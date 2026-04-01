use std::path::PathBuf;

use crate::error::IsolaError;
use crate::paths;
use crate::sandbox::config::SandboxConfig;
use crate::sandbox::namespace::{SandboxExec, enter_sandbox};

pub fn run(name: &str, workspace: Option<PathBuf>) -> Result<i32, IsolaError> {
    let rootfs = paths::rootfs_dir(name);
    if !rootfs.exists() {
        return Err(IsolaError::SandboxNotFound(name.to_string()));
    }

    let config = SandboxConfig::load(name)?;
    let workspace = workspace.or(config.workspace);

    // Resolve host SSH directory if sharing is enabled
    let ssh_dir = if config.share_ssh {
        std::env::var("HOME")
            .ok()
            .map(|h| PathBuf::from(h).join(".ssh"))
            .filter(|p| p.exists())
    } else {
        None
    };

    let exec = SandboxExec {
        rootfs: rootfs.to_string_lossy().to_string(),
        exec_path: config.shell.bin_path().to_string(),
        exec_args: config.shell.login_args(),
        env_vars: build_env_vars(true),
        workspace_host: workspace.map(|p| p.to_string_lossy().to_string()),
        ssh_dir: ssh_dir.map(|p| p.to_string_lossy().to_string()),
        run_as_uid: Some(1000u32),
        multi_uid: true,
        capture_output: false,
    };

    enter_sandbox(exec)
}

/// Enter sandbox to run a command with captured stdout+stderr (used by progress UI).
pub fn run_command_captured(
    name: &str,
    command: &str,
) -> Result<crate::sandbox::namespace::SandboxChild, IsolaError> {
    let rootfs = paths::rootfs_dir(name);
    if !rootfs.exists() {
        return Err(IsolaError::SandboxNotFound(name.to_string()));
    }

    let env_vars = build_env_vars(false);

    let exec = SandboxExec {
        rootfs: rootfs.to_string_lossy().to_string(),
        exec_path: "/bin/bash".to_string(),
        exec_args: vec!["bash".to_string(), "-c".to_string(), command.to_string()],
        env_vars,
        workspace_host: None,
        ssh_dir: None,
        run_as_uid: None,
        multi_uid: true,
        capture_output: true,
    };

    crate::sandbox::namespace::spawn_sandbox(exec)
}

/// Enter sandbox to run a command (blocking, no output capture).
pub fn run_command(name: &str, command: &str) -> Result<i32, IsolaError> {
    let rootfs = paths::rootfs_dir(name);
    if !rootfs.exists() {
        return Err(IsolaError::SandboxNotFound(name.to_string()));
    }

    let env_vars = build_env_vars(false);

    let exec = SandboxExec {
        rootfs: rootfs.to_string_lossy().to_string(),
        exec_path: "/bin/bash".to_string(),
        exec_args: vec!["bash".to_string(), "-c".to_string(), command.to_string()],
        env_vars,
        workspace_host: None,
        ssh_dir: None,
        run_as_uid: None,
        multi_uid: true,
        capture_output: false,
    };

    enter_sandbox(exec)
}

pub fn build_env_vars(as_sandbox_user: bool) -> Vec<String> {
    let mut env = if as_sandbox_user {
        vec![
            "PATH=/home/sandbox/.cargo/bin:/home/sandbox/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string(),
            "HOME=/home/sandbox".to_string(),
            "USER=sandbox".to_string(),
            "LANG=C.UTF-8".to_string(),
        ]
    } else {
        vec![
            "PATH=/root/.cargo/bin:/root/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string(),
            "HOME=/root".to_string(),
            "USER=root".to_string(),
            "LANG=C.UTF-8".to_string(),
        ]
    };

    if let Ok(term) = std::env::var("TERM") {
        env.push(format!("TERM={term}"));
    }

    // Terminal color support
    for var in &["COLORTERM", "FORCE_COLOR", "NO_COLOR", "CLICOLOR_FORCE"] {
        if let Ok(val) = std::env::var(var) {
            env.push(format!("{var}={val}"));
        }
    }

    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_vars_sandbox_user() {
        let vars = build_env_vars(true);
        assert!(vars.iter().any(|v| v == "HOME=/home/sandbox"));
        assert!(vars.iter().any(|v| v == "USER=sandbox"));
        assert!(
            vars.iter()
                .any(|v| v.starts_with("PATH=") && v.contains("/home/sandbox/.cargo/bin"))
        );
        assert!(vars.iter().any(|v| v == "LANG=C.UTF-8"));
    }

    #[test]
    fn env_vars_root_user() {
        let vars = build_env_vars(false);
        assert!(vars.iter().any(|v| v == "HOME=/root"));
        assert!(vars.iter().any(|v| v == "USER=root"));
        assert!(
            vars.iter()
                .any(|v| v.starts_with("PATH=") && v.contains("/root/.cargo/bin"))
        );
    }

    #[test]
    fn env_vars_always_has_lang() {
        for as_sandbox in [true, false] {
            let vars = build_env_vars(as_sandbox);
            assert!(vars.iter().any(|v| v == "LANG=C.UTF-8"));
        }
    }
}
