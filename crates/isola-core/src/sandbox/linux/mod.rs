pub mod cleanup;
pub mod mounts;
pub mod namespace;
pub mod seccomp;
pub mod userns;

use std::path::Path;

use crate::error::IsolaError;
use crate::paths;
use crate::sandbox::backend::SandboxBackend;

use namespace::{SandboxExec, enter_sandbox};

pub struct LinuxBackend;

impl LinuxBackend {
    /// Prepend plugin `paths.bin` directories to the PATH entry of `env_vars`
    /// so plugin binaries are runnable by name in interactive and exec sessions.
    fn add_plugin_bins_to_path(env_vars: &mut [String], environments: &[String]) {
        let bins = crate::sandbox::exec::plugin_bin_paths(environments);
        if bins.is_empty() {
            return;
        }
        for v in env_vars.iter_mut() {
            if let Some(rest) = v.strip_prefix("PATH=") {
                *v = format!("PATH={}:{rest}", bins.join(":"));
                return;
            }
        }
    }

    /// Shared package caches (from plugin `cache:` declarations) for these
    /// environments, creating the host-side directories so the in-child
    /// bind-mount succeeds.
    fn cache_mounts_for(environments: &[String]) -> Vec<(String, String)> {
        let mounts = crate::sandbox::exec::collect_cache_mounts(environments);
        for (host, _) in &mounts {
            let _ = std::fs::create_dir_all(host);
        }
        mounts
    }

    fn build_env_vars(as_sandbox_user: bool) -> Vec<String> {
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

        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            env.push(format!("ANTHROPIC_API_KEY={key}"));
        }

        for var in &["COLORTERM", "FORCE_COLOR", "NO_COLOR", "CLICOLOR_FORCE"] {
            if let Ok(val) = std::env::var(var) {
                env.push(format!("{var}={val}"));
            }
        }

        for var in &[
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
            "AWS_REGION",
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SESSION_TOKEN",
        ] {
            if let Ok(val) = std::env::var(var) {
                env.push(format!("{var}={val}"));
            }
        }

        env
    }
}

impl SandboxBackend for LinuxBackend {
    fn preflight_checks(&self) -> Result<(), IsolaError> {
        if !userns::has_uidmap_tools() {
            eprintln!(
                "Note: newuidmap/newgidmap not found (install with: sudo apt install uidmap).\n\
                 The sandbox will use single-UID mapping (no root/user separation inside)."
            );
        }

        // AppArmor (Ubuntu 24.04+) blocks unprivileged user namespaces unless a
        // profile grants them. The library reports this as an error; the CLI
        // layer offers to install the profile before reaching here.
        #[cfg(target_os = "linux")]
        if crate::host::apparmor_userns_restricted() && !crate::host::has_apparmor_profile() {
            return Err(IsolaError::NamespaceError(
                "AppArmor restricts unprivileged user namespaces on this system.\n\
                 Run `isola setup-host` (or install an AppArmor userns profile for \
                 your binary)."
                    .to_string(),
            ));
        }

        Ok(())
    }

    fn enter_interactive(
        &self,
        name: &str,
        _shell: bool,
        workspace: Option<&Path>,
        devices: Vec<String>,
    ) -> Result<i32, IsolaError> {
        let rootfs = paths::rootfs_dir(name);
        if !rootfs.exists() {
            return Err(IsolaError::SandboxNotFound(name.to_string()));
        }

        let config = crate::sandbox::config::SandboxConfig::load(name)?;

        // Collect plugin-declared host bind-mounts
        let host_mounts = crate::sandbox::exec::collect_host_mounts(&config.environments);

        let mut env_vars = Self::build_env_vars(true);
        env_vars.push(format!("ISOLA_SANDBOX={}", config.name));
        if config.share_display {
            for var in &["DISPLAY", "WAYLAND_DISPLAY", "XDG_SESSION_TYPE"] {
                if let Ok(val) = std::env::var(var) {
                    env_vars.push(format!("{var}={val}"));
                }
            }
            env_vars.push("XAUTHORITY=/home/sandbox/.Xauthority".to_string());
        }

        Self::add_plugin_bins_to_path(&mut env_vars, &config.environments);

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
            cache_mounts: Self::cache_mounts_for(&config.environments),
        };

        enter_sandbox(exec)
    }

    fn exec_command(
        &self,
        name: &str,
        command: &[String],
        workspace: Option<&Path>,
        devices: Vec<String>,
    ) -> Result<i32, IsolaError> {
        let rootfs = paths::rootfs_dir(name);
        if !rootfs.exists() {
            return Err(IsolaError::SandboxNotFound(name.to_string()));
        }

        if command.is_empty() {
            return Err(IsolaError::ConfigError("no command specified".to_string()));
        }

        let config = crate::sandbox::config::SandboxConfig::load(name)?;
        let host_mounts = crate::sandbox::exec::collect_host_mounts(&config.environments);

        let mut env_vars = Self::build_env_vars(true);
        env_vars.push(format!("ISOLA_SANDBOX={}", name));
        if config.share_display {
            for var in &["DISPLAY", "WAYLAND_DISPLAY", "XDG_SESSION_TYPE"] {
                if let Ok(val) = std::env::var(var) {
                    env_vars.push(format!("{var}={val}"));
                }
            }
            env_vars.push("XAUTHORITY=/home/sandbox/.Xauthority".to_string());
        }

        Self::add_plugin_bins_to_path(&mut env_vars, &config.environments);

        let exec = SandboxExec {
            rootfs: rootfs.to_string_lossy().to_string(),
            exec_path: command[0].clone(),
            exec_args: command.to_vec(),
            env_vars,
            workspace_host: workspace.map(|p| p.to_string_lossy().to_string()),
            host_mounts,
            share_display: config.share_display,
            run_as_uid: Some(1000),
            multi_uid: true,
            capture_output: false,
            devices,
            cache_mounts: Self::cache_mounts_for(&config.environments),
        };

        enter_sandbox(exec)
    }

    fn destroy(&self, name: &str) -> Result<(), IsolaError> {
        let sandbox_dir = paths::sandbox_dir(name);
        if !sandbox_dir.exists() {
            return Err(IsolaError::SandboxNotFound(name.to_string()));
        }

        // Enter the full sandbox namespace (user + mount + pid) and rm -rf from
        // inside. After pivot_root the process is root with full control over the
        // sandbox filesystem, so it can delete files owned by any subordinate UID.
        let rootfs = paths::rootfs_dir(name);
        if rootfs.exists() {
            let exec = SandboxExec {
                rootfs: rootfs.to_string_lossy().to_string(),
                exec_path: "/bin/sh".to_string(),
                exec_args: vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    "find / -mindepth 1 -maxdepth 1 \
                        ! -name proc ! -name sys ! -name dev ! -name tmp \
                        ! -name workspace ! -name .old_root \
                        -exec rm -rf {} + 2>/dev/null; true"
                        .to_string(),
                ],
                env_vars: Self::build_env_vars(false),
                workspace_host: None,
                host_mounts: vec![],
                share_display: false,
                run_as_uid: None,
                multi_uid: true,
                capture_output: false,
                devices: vec![],
                cache_mounts: vec![],
            };
            if let Err(e) = enter_sandbox(exec) {
                eprintln!("warning: in-sandbox cleanup failed: {e}");
                eprintln!("  (continuing with host-side deletion)");
            }
        }

        match std::fs::remove_dir_all(&sandbox_dir) {
            Ok(()) => {
                eprintln!("Sandbox '{}' destroyed", name);
                Ok(())
            }
            Err(e) => {
                eprintln!("Warning: some files could not be deleted: {e}");
                eprintln!("Try: rm -rf {}", sandbox_dir.display());
                Err(IsolaError::Io(e))
            }
        }
    }

    fn is_healthy(&self, name: &str) -> bool {
        let rootfs = paths::rootfs_dir(name);
        rootfs.join("bin").is_dir() && rootfs.join("etc").is_dir()
    }

    fn backend_name(&self) -> &'static str {
        "linux-namespace"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_vars_sandbox_user() {
        let vars = LinuxBackend::build_env_vars(true);
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
        let vars = LinuxBackend::build_env_vars(false);
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
            let vars = LinuxBackend::build_env_vars(as_sandbox);
            assert!(vars.iter().any(|v| v == "LANG=C.UTF-8"));
        }
    }
}
