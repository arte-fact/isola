use std::path::PathBuf;

use crate::error::IsolaError;
use crate::sandbox::backend;
use crate::sandbox::config::SandboxConfig;

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

/// Shared package-manager caches to bind-mount for the given environments, as
/// `(host_source_abs, sandbox_dest_abs)` pairs. Declared per plugin via the
/// `cache:` field in plugin.yaml, so downloads are reused across sandboxes and
/// sessions with no extra configuration inside the sandbox.
#[cfg(target_os = "linux")]
pub fn collect_cache_mounts(environments: &[String]) -> Vec<(String, String)> {
    let Ok(registry) = crate::plugin::PluginRegistry::load() else {
        return vec![];
    };
    let mut mounts = Vec::new();
    for env in environments {
        if let Some(plugin) = registry.get(env) {
            for c in &plugin.manifest.paths.cache {
                let host = crate::paths::pkg_cache_dir(&c.name)
                    .to_string_lossy()
                    .into_owned();
                let pair = (host, c.to.clone());
                if !mounts.contains(&pair) {
                    mounts.push(pair);
                }
            }
        }
    }
    mounts
}

/// Shared apt archives cache mounted during provisioning so downloaded `.deb`s
/// are reused across sandboxes. apt is part of the base (not a plugin) and runs
/// as in-namespace root, so this is built in rather than plugin-declared.
#[cfg(target_os = "linux")]
pub(crate) fn apt_provision_cache() -> Vec<(String, String)> {
    let apt = crate::paths::pkg_cache_dir("apt");
    let _ = std::fs::create_dir_all(&apt);
    vec![(
        apt.to_string_lossy().into_owned(),
        "/var/cache/apt/archives".to_string(),
    )]
}

/// Collect device entries from plugins and sandbox config.
pub fn collect_devices(environments: &[String], config_devices: &[String]) -> Vec<String> {
    let mut devices: Vec<String> = config_devices.to_vec();
    if let Ok(registry) = crate::plugin::PluginRegistry::load() {
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
        devices: vec![],
        cache_mounts: apt_provision_cache(),
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
        devices: vec![],
        cache_mounts: apt_provision_cache(),
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

    #[cfg(target_os = "linux")]
    #[test]
    fn cache_mounts_come_from_plugin_manifests() {
        let m = collect_cache_mounts(&["rust".to_string(), "go".to_string()]);
        let dests: Vec<&str> = m.iter().map(|(_, d)| d.as_str()).collect();
        assert!(dests.contains(&"/home/sandbox/.cargo/registry"));
        assert!(dests.contains(&"/home/sandbox/go/pkg/mod"));
        assert!(dests.contains(&"/home/sandbox/.cache/go-build"));
        // Host source lives under the shared pkg cache dir.
        assert!(m.iter().any(|(h, _)| h.contains("cache/pkg/cargo")));
        // A plugin that declares no cache contributes nothing.
        assert!(collect_cache_mounts(&["git".to_string()]).is_empty());
    }
}
