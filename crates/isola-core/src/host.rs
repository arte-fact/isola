//! Host capability detection for unprivileged user namespaces.
//!
//! These are pure, non-interactive checks shared by the CLI and library
//! consumers. The CLI layers the interactive, `sudo`-based setup on top (a
//! prompt then a re-exec); the library itself never prompts, never escalates,
//! and never touches the host — it only reports what's needed and generates the
//! AppArmor profile text for a caller to install.

#[cfg(target_os = "linux")]
const APPARMOR_PROFILE_DIR: &str = "/etc/apparmor.d";

/// Something the host is missing for isola to create sandboxes unprivileged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostRequirement {
    /// AppArmor denies unprivileged user namespaces and no isola profile exists.
    /// Either install a profile (see [`apparmor_profile_for`]) for the binary
    /// that creates the sandbox, or run as a privileged user.
    AppArmorUsernsRestricted,
}

/// Is AppArmor restricting unprivileged user namespaces? (Ubuntu 24.04+.)
#[cfg(target_os = "linux")]
pub fn apparmor_userns_restricted() -> bool {
    std::fs::read_to_string("/proc/sys/kernel/apparmor_restrict_unprivileged_userns")
        .map(|s| s.trim() == "1")
        .unwrap_or(false)
}

#[cfg(not(target_os = "linux"))]
pub fn apparmor_userns_restricted() -> bool {
    false
}

/// Is an isola AppArmor profile installed?
#[cfg(target_os = "linux")]
pub fn has_apparmor_profile() -> bool {
    std::path::Path::new(APPARMOR_PROFILE_DIR)
        .join("isola")
        .exists()
}

#[cfg(not(target_os = "linux"))]
pub fn has_apparmor_profile() -> bool {
    true
}

/// What the host is missing, if anything. An empty result means the host is
/// ready. Pure detection — no side effects, no prompts.
pub fn host_requirements() -> Vec<HostRequirement> {
    let mut reqs = Vec::new();
    if apparmor_userns_restricted() && !has_apparmor_profile() {
        reqs.push(HostRequirement::AppArmorUsernsRestricted);
    }
    reqs
}

/// Generate an AppArmor profile permitting user namespaces for `binary_path`.
///
/// The profile attaches at exec time and is matched by executable path, so a
/// **library** consumer must install this for *their own* binary (the one that
/// creates the sandbox), not for the `isola` CLI.
pub fn apparmor_profile_for(binary_path: &str) -> String {
    format!(
        r#"abi <abi/4.0>,

profile isola {binary_path} flags=(unconfined) {{
  userns,
}}
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_contains_binary_path() {
        let p = apparmor_profile_for("/usr/local/bin/isola");
        assert!(p.contains("/usr/local/bin/isola"));
    }

    #[test]
    fn profile_grants_userns_unconfined_with_abi() {
        let p = apparmor_profile_for("/usr/bin/isola");
        assert!(p.contains("userns,"));
        assert!(p.contains("flags=(unconfined)"));
        assert!(p.contains("abi <abi/4.0>"));
    }

    #[test]
    fn profile_with_spaces_in_path() {
        let p = apparmor_profile_for("/home/my user/bin/isola");
        assert!(p.contains("/home/my user/bin/isola"));
    }
}
