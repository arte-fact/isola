use std::path::PathBuf;

use crate::error::IsolaError;
use crate::paths;
use crate::sandbox::config::SandboxConfig;
use crate::sandbox::namespace::{SandboxExec, enter_sandbox};

pub fn run(
    name: &str,
    force_shell: bool,
    force_claude: Option<bool>,
    workspace: Option<PathBuf>,
) -> Result<i32, IsolaError> {
    let rootfs = paths::rootfs_dir(name);
    if !rootfs.exists() {
        return Err(IsolaError::SandboxNotFound(name.to_string()));
    }

    let config = SandboxConfig::load(name)?;
    let workspace = workspace.or(config.workspace);

    // Determine mode: explicit flags override config
    let use_claude = if force_shell {
        false
    } else if let Some(claude) = force_claude {
        claude
    } else {
        config.claude_integration
    };

    let claude_binary = if use_claude {
        find_claude_binary()
    } else {
        None
    };

    // Resolve host SSH directory if sharing is enabled
    let ssh_dir = if config.share_ssh {
        std::env::var("HOME")
            .ok()
            .map(|h| PathBuf::from(h).join(".ssh"))
            .filter(|p| p.exists())
    } else {
        None
    };

    // Only bind-mount credentials when Claude integration is active
    let creds_source = if use_claude {
        let host_creds = paths::host_claude_credentials();
        let source = if host_creds.exists() {
            host_creds
        } else {
            let session = paths::session_credentials();
            std::fs::create_dir_all(paths::session_dir())?;
            if !session.exists() {
                std::fs::File::create(&session)?;
            }
            session
        };

        // Ensure bind-mount target exists in rootfs
        let creds_target = rootfs.join("home/sandbox/.claude/.credentials.json");
        if !creds_target.exists() {
            std::fs::create_dir_all(rootfs.join("home/sandbox/.claude"))?;
            std::fs::File::create(&creds_target)?;
        }

        Some(source)
    } else {
        None
    };

    let (exec_path, exec_args, run_as_uid, env_vars) = if use_claude {
        // Claude mode: sandbox user + --dangerously-skip-permissions
        let claude = claude_binary
            .as_ref()
            .map(|_| "/usr/local/bin/claude".to_string())
            .unwrap_or_else(|| {
                eprintln!("Warning: Claude binary not found on host, falling back to shell");
                config.shell.bin_path().to_string()
            });
        if claude.contains("claude") {
            (
                claude,
                vec![
                    "claude".to_string(),
                    "--dangerously-skip-permissions".to_string(),
                ],
                Some(1000u32),
                build_env_vars(true, true),
            )
        } else {
            (
                claude,
                config.shell.login_args(),
                Some(1000u32),
                build_env_vars(true, false),
            )
        }
    } else {
        // Shell mode (default)
        (
            config.shell.bin_path().to_string(),
            config.shell.login_args(),
            Some(1000u32),
            build_env_vars(true, false),
        )
    };

    let exec = SandboxExec {
        rootfs: rootfs.to_string_lossy().to_string(),
        exec_path,
        exec_args,
        env_vars,
        workspace_host: workspace.map(|p| p.to_string_lossy().to_string()),
        claude_binary: claude_binary.map(|p| p.to_string_lossy().to_string()),
        session_credentials: creds_source.map(|p| p.to_string_lossy().to_string()),
        ssh_dir: ssh_dir.map(|p| p.to_string_lossy().to_string()),
        run_as_uid,
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

    let env_vars = build_env_vars(false, false);

    let exec = SandboxExec {
        rootfs: rootfs.to_string_lossy().to_string(),
        exec_path: "/bin/bash".to_string(),
        exec_args: vec!["bash".to_string(), "-c".to_string(), command.to_string()],
        env_vars,
        workspace_host: None,
        claude_binary: None,
        session_credentials: None,
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

    let env_vars = build_env_vars(false, false);

    let exec = SandboxExec {
        rootfs: rootfs.to_string_lossy().to_string(),
        exec_path: "/bin/bash".to_string(),
        exec_args: vec!["bash".to_string(), "-c".to_string(), command.to_string()],
        env_vars,
        workspace_host: None,
        claude_binary: None,
        session_credentials: None,
        ssh_dir: None,
        run_as_uid: None,
        multi_uid: true,
        capture_output: false,
    };

    enter_sandbox(exec)
}

pub fn find_claude_binary() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let candidates = [
        PathBuf::from(&home).join(".local/bin/claude"),
        PathBuf::from("/usr/local/bin/claude"),
        PathBuf::from("/usr/bin/claude"),
    ];
    for path in &candidates {
        if path.exists() {
            if let Ok(resolved) = std::fs::canonicalize(path) {
                return Some(resolved);
            }
            return Some(path.clone());
        }
    }
    if let Ok(output) = std::process::Command::new("which").arg("claude").output()
        && output.status.success()
    {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if let Ok(resolved) = std::fs::canonicalize(&path) {
            return Some(resolved);
        }
        return Some(PathBuf::from(path));
    }
    None
}

pub fn build_env_vars(as_sandbox_user: bool, include_claude_vars: bool) -> Vec<String> {
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

    // Claude-specific vars (only when integration enabled)
    if include_claude_vars {
        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            env.push(format!("ANTHROPIC_API_KEY={key}"));
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
    }

    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_vars_sandbox_user() {
        let vars = build_env_vars(true, false);
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
        let vars = build_env_vars(false, false);
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
            let vars = build_env_vars(as_sandbox, false);
            assert!(vars.iter().any(|v| v == "LANG=C.UTF-8"));
        }
    }
}
