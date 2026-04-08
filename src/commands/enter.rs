use std::path::PathBuf;

use crate::error::IsolaError;
use crate::sandbox::backend;
use crate::sandbox::config::SandboxConfig;

pub fn run(name: &str, workspace: Option<PathBuf>) -> Result<i32, IsolaError> {
    let config = SandboxConfig::load(name)?;
    let workspace = workspace.or(config.workspace);

    let b = backend::create_backend();
    b.enter_interactive(name, false, workspace.as_deref())
}

/// Collect host_mount entries from the given environments' plugins.
#[cfg(target_os = "linux")]
pub fn collect_host_mounts(environments: &[String]) -> Vec<(String, String, bool)> {
    use crate::plugin::PluginRegistry;

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

/// Enter sandbox to run a command with captured stdout+stderr (used by progress UI).
#[cfg(target_os = "linux")]
pub fn run_command_captured(
    name: &str,
    command: &str,
) -> Result<crate::sandbox::linux::namespace::SandboxChild, IsolaError> {
    let rootfs = crate::paths::rootfs_dir(name);
    if !rootfs.exists() {
        return Err(IsolaError::SandboxNotFound(name.to_string()));
    }

    let env_vars = build_env_vars(false);

    let exec = crate::sandbox::linux::namespace::SandboxExec {
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
    };

    crate::sandbox::linux::namespace::spawn_sandbox(exec)
}

/// Enter sandbox to run a command (blocking, no output capture).
#[cfg(target_os = "linux")]
pub fn run_command(name: &str, command: &str) -> Result<i32, IsolaError> {
    let rootfs = crate::paths::rootfs_dir(name);
    if !rootfs.exists() {
        return Err(IsolaError::SandboxNotFound(name.to_string()));
    }

    let env_vars = build_env_vars(false);

    let exec = crate::sandbox::linux::namespace::SandboxExec {
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
    };

    crate::sandbox::linux::namespace::enter_sandbox(exec)
}

#[cfg(target_os = "linux")]
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
    #[cfg(target_os = "linux")]
    use super::*;

    #[cfg(target_os = "linux")]
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

    #[cfg(target_os = "linux")]
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

    #[cfg(target_os = "linux")]
    #[test]
    fn env_vars_always_has_lang() {
        for as_sandbox in [true, false] {
            let vars = build_env_vars(as_sandbox);
            assert!(vars.iter().any(|v| v == "LANG=C.UTF-8"));
        }
    }
}
