use std::path::PathBuf;

use crate::error::IsolaError;
use crate::paths;
use crate::sandbox::config::SandboxConfig;
use crate::sandbox::namespace::{SandboxExec, enter_sandbox};

pub fn run(name: &str, shell: bool, workspace: Option<PathBuf>) -> Result<i32, IsolaError> {
    let rootfs = paths::rootfs_dir(name);
    if !rootfs.exists() {
        return Err(IsolaError::SandboxNotFound(name.to_string()));
    }

    let config = SandboxConfig::load(name)?;
    let workspace = workspace.or(config.workspace);

    let claude_binary = find_claude_binary();

    // Ensure shared session directory exists
    let session_credentials = paths::session_credentials();
    std::fs::create_dir_all(paths::session_dir())?;

    // Auto-import host credentials if session file is empty or missing
    let session_has_content = session_credentials
        .metadata()
        .map(|m| m.len() > 0)
        .unwrap_or(false);
    if !session_has_content {
        let host_creds = paths::host_claude_credentials();
        if let Ok(data) = std::fs::read(&host_creds) {
            if !data.is_empty() {
                std::fs::write(&session_credentials, &data)?;
                eprintln!(
                    "Imported Claude session from {}",
                    host_creds.display()
                );
            }
        }
    }

    // Create session file if it still doesn't exist (needed as bind-mount source)
    if !session_credentials.exists() {
        std::fs::File::create(&session_credentials)?;
    }

    // Ensure bind-mount target exists in rootfs
    let creds_target = rootfs.join("home/sandbox/.claude/.credentials.json");
    if !creds_target.exists() {
        std::fs::create_dir_all(rootfs.join("home/sandbox/.claude"))?;
        std::fs::File::create(&creds_target)?;
    }

    // Only bind-mount credentials if the session file has content.
    // An empty bind-mount would hide credentials Claude Code wrote
    // directly into the rootfs.
    let mount_session_credentials = session_credentials
        .metadata()
        .map(|m| m.len() > 0)
        .unwrap_or(false);

    let (exec_path, exec_args, run_as_uid, env_vars) = if shell {
        // Shell mode: sandbox user
        (
            "/bin/bash".to_string(),
            vec!["bash".to_string(), "-l".to_string()],
            Some(1000u32),
            build_env_vars(true),
        )
    } else {
        // Claude mode: sandbox user + --dangerously-skip-permissions
        let claude = claude_binary
            .as_ref()
            .map(|_| "/usr/local/bin/claude".to_string())
            .unwrap_or_else(|| {
                eprintln!("Warning: Claude binary not found on host, falling back to shell");
                "/bin/bash".to_string()
            });
        if claude.contains("claude") {
            (
                claude,
                vec![
                    "claude".to_string(),
                    "--dangerously-skip-permissions".to_string(),
                ],
                Some(1000u32),
                build_env_vars(true),
            )
        } else {
            (
                claude,
                vec!["bash".to_string()],
                None,
                build_env_vars(false),
            )
        }
    };

    let exec = SandboxExec {
        rootfs: rootfs.to_string_lossy().to_string(),
        exec_path,
        exec_args,
        env_vars,
        workspace_host: workspace.map(|p| p.to_string_lossy().to_string()),
        claude_binary: claude_binary.map(|p| p.to_string_lossy().to_string()),
        session_credentials: if mount_session_credentials {
            Some(session_credentials.to_string_lossy().to_string())
        } else {
            None
        },
        run_as_uid,
        multi_uid: true,
    };

    enter_sandbox(exec)
}

/// Enter sandbox to run an arbitrary command as root (used by provisioning)
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
        claude_binary: None,
        session_credentials: None,
        run_as_uid: None,
        multi_uid: true,
    };

    enter_sandbox(exec)
}

fn find_claude_binary() -> Option<PathBuf> {
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
    if let Ok(output) = std::process::Command::new("which").arg("claude").output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if let Ok(resolved) = std::fs::canonicalize(&path) {
                return Some(resolved);
            }
            return Some(PathBuf::from(path));
        }
    }
    None
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

    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        env.push(format!("ANTHROPIC_API_KEY={key}"));
    }

    // Terminal color support
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
