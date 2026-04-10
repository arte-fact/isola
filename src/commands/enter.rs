use std::path::PathBuf;

use crate::error::IsolaError;
use crate::paths;
use crate::plugin::PluginRegistry;
use crate::sandbox::config::SandboxConfig;
use crate::sandbox::namespace::{SandboxExec, enter_sandbox};

pub fn run(
    name: &str,
    workspace: Option<PathBuf>,
    cli_devices: Vec<String>,
) -> Result<i32, IsolaError> {
    let rootfs = paths::rootfs_dir(name);
    if !rootfs.exists() {
        return Err(IsolaError::SandboxNotFound(name.to_string()));
    }

    let config = SandboxConfig::load(name)?;
    let workspace = workspace.or(config.workspace);

    // Collect plugin-declared host bind-mounts (includes ssh-keys, git-config, etc.)
    let host_mounts = collect_host_mounts(&config.environments);
    let mut devices = collect_devices(&config.environments, &config.devices);
    for d in cli_devices {
        if !devices.contains(&d) {
            devices.push(d);
        }
    }

    let mut env_vars = build_env_vars(true);
    env_vars.push(format!("ISOLA_SANDBOX={}", config.name));
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
        exec_path: config.shell.bin_path().to_string(),
        exec_args: config.shell.login_args(),
        env_vars,
        workspace_host: workspace.map(|p| p.to_string_lossy().to_string()),
        host_mounts,
        share_display: config.share_display,
        run_as_uid: Some(1000u32),
        multi_uid: true,
        capture_output: false,
        devices,
    };

    enter_sandbox(exec)
}

/// Collect host_mount entries from the given environments' plugins.
pub fn collect_host_mounts(environments: &[String]) -> Vec<(String, String, bool)> {
    let Ok(registry) = PluginRegistry::load() else {
        return vec![];
    };
    environments
        .iter()
        .filter_map(|e| registry.get(e))
        .flat_map(|p| {
            p.manifest
                .paths
                .host_mount
                .iter()
                .map(|m| (m.from.clone(), m.to.clone(), m.readonly))
        })
        .collect()
}

/// Collect device entries from plugins and sandbox config.
pub fn collect_devices(environments: &[String], config_devices: &[String]) -> Vec<String> {
    let mut devices: Vec<String> = config_devices.to_vec();
    if let Ok(registry) = PluginRegistry::load() {
        for env in environments {
            if let Some(plugin) = registry.get(env) {
                for d in &plugin.manifest.paths.device {
                    if !devices.contains(&d.path) {
                        devices.push(d.path.clone());
                    }
                }
            }
        }
    }
    devices
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
        host_mounts: vec![],
        share_display: false,
        run_as_uid: None,
        multi_uid: true,
        capture_output: true,
        devices: vec![],
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
        host_mounts: vec![],
        share_display: false,
        run_as_uid: None,
        multi_uid: true,
        capture_output: false,
        devices: vec![],
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
            "XDG_RUNTIME_DIR=/run/user/1000".to_string(),
        ]
    } else {
        vec![
            "PATH=/root/.cargo/bin:/root/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string(),
            "HOME=/root".to_string(),
            "USER=root".to_string(),
            "LANG=C.UTF-8".to_string(),
            "XDG_RUNTIME_DIR=/run/user/0".to_string(),
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
        assert!(vars.iter().any(|v| v == "XDG_RUNTIME_DIR=/run/user/1000"));
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
        assert!(vars.iter().any(|v| v == "XDG_RUNTIME_DIR=/run/user/0"));
    }

    #[test]
    fn env_vars_always_has_lang() {
        for as_sandbox in [true, false] {
            let vars = build_env_vars(as_sandbox);
            assert!(vars.iter().any(|v| v == "LANG=C.UTF-8"));
        }
    }
}
