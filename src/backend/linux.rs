use std::path::{Path, PathBuf};

use crate::error::IsolaError;
use crate::paths;
use crate::sandbox::namespace::{SandboxExec, enter_sandbox as ns_enter_sandbox};
use crate::sandbox::{rootfs, userns};

// --- Preflight / Host Setup ---

const APPARMOR_PROFILE_DIR: &str = "/etc/apparmor.d";

fn apparmor_userns_restricted() -> bool {
    std::fs::read_to_string("/proc/sys/kernel/apparmor_restrict_unprivileged_userns")
        .map(|s| s.trim() == "1")
        .unwrap_or(false)
}

fn has_apparmor_profile() -> bool {
    Path::new(APPARMOR_PROFILE_DIR).join("isola").exists()
}

pub fn preflight_checks() -> Result<(), IsolaError> {
    if !userns::has_uidmap_tools() {
        eprintln!(
            "Note: newuidmap/newgidmap not found (install with: sudo apt install uidmap).\n\
             The sandbox will use single-UID mapping (no root/user separation inside)."
        );
    }

    if apparmor_userns_restricted() && !has_apparmor_profile() {
        return Err(IsolaError::NamespaceError(
            "AppArmor restricts unprivileged user namespaces on this system.\n\
             Run `isola setup-host` to install the required AppArmor profile."
                .to_string(),
        ));
    }

    Ok(())
}

pub fn setup_host() -> Result<(), IsolaError> {
    if !apparmor_userns_restricted() {
        eprintln!("AppArmor user namespace restriction is not active — no profile needed.");
        return Ok(());
    }

    if has_apparmor_profile() {
        eprintln!("AppArmor profile for isola is already installed.");
        return Ok(());
    }

    let binary_path = std::env::current_exe()
        .and_then(|p| std::fs::canonicalize(&p))
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| {
            IsolaError::ConfigError(format!("Could not determine isola binary path: {e}"))
        })?;

    let profile = format!(
        r#"abi <abi/4.0>,

profile isola {binary_path} flags=(unconfined) {{
  userns,
}}
"#
    );

    eprintln!("Installing AppArmor profile for: {binary_path}");
    eprintln!("This requires sudo to write to {APPARMOR_PROFILE_DIR}/isola");

    let status = std::process::Command::new("sudo")
        .args(["tee", &format!("{APPARMOR_PROFILE_DIR}/isola")])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(ref mut stdin) = child.stdin {
                stdin.write_all(profile.as_bytes())?;
            }
            child.wait()
        })
        .map_err(|e| IsolaError::ConfigError(format!("Failed to write AppArmor profile: {e}")))?;

    if !status.success() {
        return Err(IsolaError::ConfigError(
            "Failed to install AppArmor profile".into(),
        ));
    }

    let reload = std::process::Command::new("sudo")
        .args([
            "apparmor_parser",
            "-r",
            &format!("{APPARMOR_PROFILE_DIR}/isola"),
        ])
        .status()
        .map_err(|e| IsolaError::ConfigError(format!("Failed to reload AppArmor profile: {e}")))?;

    if !reload.success() {
        return Err(IsolaError::ConfigError(
            "Failed to reload AppArmor profile. Try: sudo apparmor_parser -r /etc/apparmor.d/isola"
                .into(),
        ));
    }

    eprintln!("AppArmor profile installed and loaded successfully.");
    Ok(())
}

// --- Sandbox Operations ---

pub fn create_sandbox(
    name: &str,
    _workspace: Option<&Path>,
    environments: &[String],
) -> Result<(), IsolaError> {
    let tarball = rootfs::ensure_rootfs_cached()?;
    let rootfs_path = paths::rootfs_dir(name);
    std::fs::create_dir_all(&rootfs_path)?;
    rootfs::extract_rootfs(&tarball, &rootfs_path)?;
    rootfs::post_setup_rootfs(&rootfs_path, name, environments)?;
    Ok(())
}

pub fn enter_sandbox(
    name: &str,
    shell: bool,
    workspace: Option<PathBuf>,
) -> Result<i32, IsolaError> {
    let rootfs = paths::rootfs_dir(name);
    if !rootfs.exists() {
        return Err(IsolaError::SandboxNotFound(name.to_string()));
    }

    let claude_binary = find_claude_binary();

    // Ensure shared session directory and credentials file exist
    let session_credentials = paths::session_credentials();
    std::fs::create_dir_all(paths::session_dir())?;
    if !session_credentials.exists() {
        std::fs::File::create(&session_credentials)?;
    }
    // Ensure bind-mount target exists in rootfs
    let creds_target = rootfs.join("home/sandbox/.claude/.credentials.json");
    if !creds_target.exists() {
        std::fs::create_dir_all(rootfs.join("home/sandbox/.claude"))?;
        std::fs::File::create(&creds_target)?;
    }

    let (exec_path, exec_args, run_as_uid, env_vars) = if shell {
        // Shell mode: root for full admin access
        (
            "/bin/bash".to_string(),
            vec!["bash".to_string()],
            None,
            build_env_vars(false),
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
        session_credentials: Some(session_credentials.to_string_lossy().to_string()),
        run_as_uid,
        multi_uid: true,
    };

    ns_enter_sandbox(exec)
}

pub fn exec_command(
    name: &str,
    command: Vec<String>,
    workspace: Option<PathBuf>,
) -> Result<i32, IsolaError> {
    let rootfs = paths::rootfs_dir(name);
    if !rootfs.exists() {
        return Err(IsolaError::SandboxNotFound(name.to_string()));
    }

    if command.is_empty() {
        return Err(IsolaError::ConfigError("no command specified".to_string()));
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

    ns_enter_sandbox(exec)
}

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

    ns_enter_sandbox(exec)
}

pub fn destroy_sandbox(name: &str) -> Result<(), IsolaError> {
    let sandbox_dir = paths::sandbox_dir(name);
    let rootfs = paths::rootfs_dir(name);

    // Files may be owned by subordinate UIDs. Use a lightweight user namespace
    // to chown everything back to our host UID before deleting.
    if rootfs.exists() {
        let rootfs_str = rootfs.to_string_lossy().to_string();
        let _ = userns::run_in_userns(
            move || {
                let _ = std::process::Command::new("chown")
                    .args(["-R", "0:0", &rootfs_str])
                    .status();
                0
            },
            false,
        );
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

pub fn reprovision(name: &str, environments: &[String]) -> Result<(), IsolaError> {
    let rootfs_path = paths::rootfs_dir(name);
    if !rootfs_path.exists() {
        return Err(IsolaError::SandboxNotFound(name.to_string()));
    }

    rootfs::post_setup_rootfs(&rootfs_path, name, environments)?;

    let script = rootfs::build_provision_script(environments);
    eprintln!("Re-provisioning '{}': {}...", name, environments.join(", "));
    let exit_code = run_command(name, &script)?;
    if exit_code != 0 {
        return Err(IsolaError::ProvisionFailed(exit_code));
    }

    eprintln!("Sandbox '{}' re-provisioned successfully!", name);
    Ok(())
}

// --- Helper Functions ---

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
