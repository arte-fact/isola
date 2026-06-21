#[cfg(target_os = "linux")]
use std::path::Path;

use crate::error::IsolaError;

#[cfg(target_os = "linux")]
const APPARMOR_PROFILE_DIR: &str = "/etc/apparmor.d";

/// Check if AppArmor user namespace restriction is active
#[cfg(target_os = "linux")]
pub fn apparmor_userns_restricted() -> bool {
    std::fs::read_to_string("/proc/sys/kernel/apparmor_restrict_unprivileged_userns")
        .map(|s| s.trim() == "1")
        .unwrap_or(false)
}

/// Check if isola already has an AppArmor profile installed
#[cfg(target_os = "linux")]
pub fn has_apparmor_profile() -> bool {
    Path::new(APPARMOR_PROFILE_DIR).join("isola").exists()
}

/// Ensure unprivileged user namespaces are usable before creating a sandbox.
///
/// On systems where AppArmor blocks them (Ubuntu 24.04+) and no isola profile
/// is installed yet, offer — interactively — to run host setup now, then re-exec
/// to pick up the freshly installed profile (it only attaches at the next exec).
/// `setup-host` needs `sudo`, so this is offered rather than done silently.
/// Returns `Err` if the blocker remains: setup was declined, we're not on a
/// terminal, or it failed.
#[cfg(target_os = "linux")]
pub fn ensure_userns_allowed() -> Result<(), IsolaError> {
    use std::io::IsTerminal;

    // Not blocked (or already fixed) → nothing to do.
    if !apparmor_userns_restricted() || has_apparmor_profile() {
        return Ok(());
    }

    let blocked = || {
        IsolaError::NamespaceError(
            "AppArmor restricts unprivileged user namespaces on this system.\n\
             Run `isola setup-host` to install the required AppArmor profile."
                .to_string(),
        )
    };

    // Can only offer if we can actually prompt (and read sudo's password prompt).
    if !(std::io::stdin().is_terminal() && std::io::stderr().is_terminal()) {
        return Err(blocked());
    }

    eprintln!(
        "AppArmor on this system blocks the unprivileged user namespaces isola needs.\n\
         isola can install a one-time AppArmor profile (and set up subordinate UID\n\
         ranges) now — this uses `sudo`."
    );
    let proceed = inquire::Confirm::new("Run host setup now?")
        .with_default(true)
        .prompt()
        .unwrap_or(false);
    if !proceed {
        return Err(blocked());
    }

    run()?;
    if !has_apparmor_profile() {
        return Err(blocked());
    }

    // The new profile only attaches at exec time, so re-exec isola with the same
    // arguments to pick it up and continue without the user re-running anything.
    use std::os::unix::process::CommandExt;
    let exec_err = std::process::Command::new("/proc/self/exe")
        .args(std::env::args_os().skip(1))
        .exec();
    eprintln!(
        "Host setup complete, but isola couldn't restart itself ({exec_err}).\n\
         Please re-run your command."
    );
    std::process::exit(0);
}

/// Generate AppArmor profile content for the isola binary at the given path
#[cfg(target_os = "linux")]
pub(crate) fn generate_profile(binary_path: &str) -> String {
    format!(
        r#"abi <abi/4.0>,

profile isola {binary_path} flags=(unconfined) {{
  userns,
}}
"#
    )
}

/// Find the isola binary path (the currently running executable)
#[cfg(target_os = "linux")]
fn find_binary_path() -> Result<String, IsolaError> {
    std::env::current_exe()
        .and_then(|p| std::fs::canonicalize(&p))
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| IsolaError::ConfigError(format!("Could not determine isola binary path: {e}")))
}

pub fn run() -> Result<(), IsolaError> {
    #[cfg(target_os = "linux")]
    {
        eprintln!("=== isola host setup ===\n");
        let mut all_ok = true;

        // Step 1: make the bundled isola-uidmap helper setuid root (enables
        // multi-UID mapping without the system `uidmap` package).
        if let Err(e) = setup_uidmap_helper() {
            eprintln!("  [SKIP] uidmap helper: {e}");
            all_ok = false;
        }

        // Step 2: subordinate UID/GID ranges in /etc/subuid and /etc/subgid.
        if let Err(e) = setup_subordinate_ids() {
            eprintln!("  [SKIP] subordinate IDs: {e}");
            all_ok = false;
        }

        // Step 3: AppArmor profile (Ubuntu 24.04+ restricts unprivileged userns).
        if let Err(e) = setup_apparmor() {
            eprintln!("  [SKIP] AppArmor: {e}");
            all_ok = false;
        }

        eprintln!();
        if all_ok {
            eprintln!("All checks passed. You can now run: isola create <name>");
        } else {
            eprintln!(
                "Some steps were skipped (see above). You can re-run `isola setup-host` anytime."
            );
        }
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("setup-host is only needed on Linux.");
        Ok(())
    }
}

/// Locate the bundled `isola-uidmap` binary next to the isola executable,
/// regardless of whether it is setuid yet.
#[cfg(target_os = "linux")]
fn uidmap_helper_path() -> Option<std::path::PathBuf> {
    let candidate = std::env::current_exe().ok()?.parent()?.join("isola-uidmap");
    candidate.exists().then_some(candidate)
}

/// Make the bundled `isola-uidmap` binary setuid root so it can write multi-line
/// UID/GID maps without the system `uidmap` package.
#[cfg(target_os = "linux")]
fn setup_uidmap_helper() -> Result<(), IsolaError> {
    use std::os::unix::fs::PermissionsExt;

    let helper = uidmap_helper_path().ok_or_else(|| {
        IsolaError::ConfigError("isola-uidmap binary not found next to the isola binary".into())
    })?;

    if std::fs::metadata(&helper)
        .map(|m| m.permissions().mode() & 0o4000 != 0)
        .unwrap_or(false)
    {
        eprintln!("  [OK] uidmap helper already setuid: {}", helper.display());
        return Ok(());
    }

    eprintln!("  Making isola-uidmap setuid root (requires sudo)...");
    let h = helper.display().to_string();
    let status = std::process::Command::new("sudo")
        .args([
            "sh",
            "-c",
            &format!("chown root:root '{h}' && chmod u+s '{h}'"),
        ])
        .status()
        .map_err(|e| IsolaError::ConfigError(format!("Failed to run sudo: {e}")))?;
    if !status.success() {
        return Err(IsolaError::ConfigError(format!(
            "Failed to make {h} setuid root. Run manually: sudo chown root:root '{h}' && sudo chmod u+s '{h}'"
        )));
    }
    eprintln!("  [OK] uidmap helper is now setuid root");
    Ok(())
}

/// Check whether /etc/subuid or /etc/subgid has an entry for the given user.
#[cfg(target_os = "linux")]
fn has_subordinate_entry(path: &str, username: &str, uid: u32) -> bool {
    let uid_str = uid.to_string();
    std::fs::read_to_string(path)
        .map(|content| {
            content.lines().any(|line| {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    return false;
                }
                let parts: Vec<&str> = line.split(':').collect();
                parts.len() >= 3 && (parts[0] == username || parts[0] == uid_str)
            })
        })
        .unwrap_or(false)
}

/// Add subordinate UID/GID ranges to /etc/subuid and /etc/subgid for the user.
#[cfg(target_os = "linux")]
fn setup_subordinate_ids() -> Result<(), IsolaError> {
    let username = std::env::var("USER").unwrap_or_else(|_| String::from("unknown"));
    let uid = nix::unistd::getuid().as_raw();

    if has_subordinate_entry("/etc/subuid", &username, uid)
        && has_subordinate_entry("/etc/subgid", &username, uid)
    {
        eprintln!("  [OK] subordinate UID/GID ranges already configured");
        return Ok(());
    }

    let (start, count) = (100000u32, 65536u32);
    let range = format!("{start}-{}", start + count - 1);
    eprintln!("  Adding subordinate IDs for '{username}' (requires sudo)...");
    let status = std::process::Command::new("sudo")
        .args([
            "usermod",
            "--add-subuids",
            &range,
            "--add-subgids",
            &range,
            &username,
        ])
        .status()
        .map_err(|e| IsolaError::ConfigError(format!("Failed to run sudo usermod: {e}")))?;
    if !status.success() {
        return Err(IsolaError::ConfigError(format!(
            "Failed to add subordinate IDs. Run manually:\n  \
             sudo usermod --add-subuids {range} --add-subgids {range} {username}"
        )));
    }
    eprintln!("  [OK] subordinate UID/GID ranges configured");
    Ok(())
}

/// Install and load the AppArmor profile if unprivileged userns is restricted.
#[cfg(target_os = "linux")]
fn setup_apparmor() -> Result<(), IsolaError> {
    if !apparmor_userns_restricted() {
        eprintln!("  [OK] AppArmor userns restriction not active — no profile needed");
        return Ok(());
    }
    if has_apparmor_profile() {
        eprintln!("  [OK] AppArmor profile already installed");
        return Ok(());
    }

    let binary_path = find_binary_path()?;
    let profile = generate_profile(&binary_path);
    eprintln!("  Installing AppArmor profile for {binary_path} (requires sudo)...");

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
    eprintln!("  [OK] AppArmor profile installed and loaded");
    Ok(())
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn profile_contains_binary_path() {
        let profile = generate_profile("/usr/local/bin/isola");
        assert!(profile.contains("/usr/local/bin/isola"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn profile_contains_userns_permission() {
        let profile = generate_profile("/usr/local/bin/isola");
        assert!(profile.contains("userns,"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn profile_contains_abi_declaration() {
        let profile = generate_profile("/usr/local/bin/isola");
        assert!(profile.contains("abi <abi/4.0>"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn profile_with_spaces_in_path() {
        let profile = generate_profile("/home/my user/bin/isola");
        assert!(profile.contains("/home/my user/bin/isola"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn profile_has_unconfined_flag() {
        let profile = generate_profile("/usr/bin/isola");
        assert!(profile.contains("flags=(unconfined)"));
    }
}
