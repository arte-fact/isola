use std::path::Path;

use crate::error::IsolaError;

const APPARMOR_PROFILE_DIR: &str = "/etc/apparmor.d";

/// Check if AppArmor user namespace restriction is active
pub fn apparmor_userns_restricted() -> bool {
    std::fs::read_to_string("/proc/sys/kernel/apparmor_restrict_unprivileged_userns")
        .map(|s| s.trim() == "1")
        .unwrap_or(false)
}

/// Check if isola already has an AppArmor profile installed
pub fn has_apparmor_profile() -> bool {
    Path::new(APPARMOR_PROFILE_DIR).join("isola").exists()
}

/// Generate AppArmor profile content for the isola binary at the given path
fn generate_profile(binary_path: &str) -> String {
    format!(
        r#"abi <abi/4.0>,

profile isola {binary_path} flags=(unconfined) {{
  userns,
}}
"#
    )
}

/// Find the isola binary path (the currently running executable)
fn find_binary_path() -> Result<String, IsolaError> {
    std::env::current_exe()
        .and_then(|p| std::fs::canonicalize(&p))
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| IsolaError::ConfigError(format!("Could not determine isola binary path: {e}")))
}

pub fn run() -> Result<(), IsolaError> {
    if !apparmor_userns_restricted() {
        eprintln!("AppArmor user namespace restriction is not active — no profile needed.");
        return Ok(());
    }

    if has_apparmor_profile() {
        eprintln!("AppArmor profile for isola is already installed.");
        return Ok(());
    }

    let binary_path = find_binary_path()?;
    let profile = generate_profile(&binary_path);

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

    // Reload the profile
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
