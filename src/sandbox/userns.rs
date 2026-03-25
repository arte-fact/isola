use crate::error::IsolaError;

/// Check whether newuidmap and newgidmap are available on this system.
pub fn has_uidmap_tools() -> bool {
    ["newuidmap", "newgidmap"].iter().all(|bin| {
        std::process::Command::new("which")
            .arg(bin)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

/// Parse /etc/subuid or /etc/subgid for the current user, return (start, count)
pub fn parse_subordinate_ids(path: &str) -> Result<(u32, u32), IsolaError> {
    let username = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "nobody".to_string());
    let uid_str = nix::unistd::getuid().as_raw().to_string();

    let content = std::fs::read_to_string(path)
        .map_err(|e| IsolaError::NamespaceError(format!("Failed to read {path}: {e}")))?;

    for line in content.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() >= 3 && (parts[0] == username || parts[0] == uid_str) {
            let start: u32 = parts[1].parse().map_err(|_| {
                IsolaError::NamespaceError(format!("Invalid subordinate ID in {path}"))
            })?;
            let count: u32 = parts[2].parse().map_err(|_| {
                IsolaError::NamespaceError(format!("Invalid subordinate count in {path}"))
            })?;
            return Ok((start, count));
        }
    }

    Err(IsolaError::NamespaceError(format!(
        "No subordinate ID range found for user '{username}' in {path}. \
         Add an entry with: sudo usermod --add-subuids 100000-165535 --add-subgids 100000-165535 {username}"
    )))
}

/// Write UID/GID mappings for a child process in a new user namespace.
/// If `multi_uid` is true, tries newuidmap/newgidmap for full UID range.
/// Returns `true` if multi-UID mapping was applied, `false` if single-UID fallback was used.
pub fn write_id_mappings(child_pid: i32, multi_uid: bool) -> Result<bool, IsolaError> {
    let uid = nix::unistd::getuid().as_raw();
    let gid = nix::unistd::getgid().as_raw();

    if multi_uid && has_uidmap_tools() {
        let (sub_uid_start, sub_uid_count) = parse_subordinate_ids("/etc/subuid")?;
        let (sub_gid_start, sub_gid_count) = parse_subordinate_ids("/etc/subgid")?;

        let pid_str = child_pid.to_string();

        // Map UID 1000 (sandbox user) to the host user's real UID so that
        // files created in /workspace are owned by the host user.
        //   Inside 0       → sub_uid_start          [1]
        //   Inside 1       → sub_uid_start+1        [999]
        //   Inside 1000    → host_uid               [1]
        //   Inside 1001    → sub_uid_start+1000     [remaining]
        let uid_remaining = sub_uid_count.saturating_sub(1000).min(64535);
        let uid_status = std::process::Command::new("newuidmap")
            .args([
                &pid_str,
                "0",
                &sub_uid_start.to_string(),
                "1",
                "1",
                &(sub_uid_start + 1).to_string(),
                "999",
                "1000",
                &uid.to_string(),
                "1",
                "1001",
                &(sub_uid_start + 1000).to_string(),
                &uid_remaining.to_string(),
            ])
            .status()
            .map_err(|e| {
                IsolaError::NamespaceError(format!(
                    "Failed to run newuidmap: {e}. Install with: sudo apt install uidmap"
                ))
            })?;

        if !uid_status.success() {
            return Err(IsolaError::NamespaceError(
                "newuidmap failed. Check /etc/subuid configuration.".into(),
            ));
        }

        let gid_remaining = sub_gid_count.saturating_sub(1000).min(64535);
        let gid_status = std::process::Command::new("newgidmap")
            .args([
                &pid_str,
                "0",
                &sub_gid_start.to_string(),
                "1",
                "1",
                &(sub_gid_start + 1).to_string(),
                "999",
                "1000",
                &gid.to_string(),
                "1",
                "1001",
                &(sub_gid_start + 1000).to_string(),
                &gid_remaining.to_string(),
            ])
            .status()
            .map_err(|e| {
                IsolaError::NamespaceError(format!(
                    "Failed to run newgidmap: {e}. Install with: sudo apt install uidmap"
                ))
            })?;

        if !gid_status.success() {
            return Err(IsolaError::NamespaceError(
                "newgidmap failed. Check /etc/subgid configuration.".into(),
            ));
        }
        return Ok(true);
    }

    if multi_uid {
        eprintln!("Warning: newuidmap/newgidmap not found, falling back to single-UID mapping");
    }

    std::fs::write(format!("/proc/{child_pid}/setgroups"), "deny\n")?;
    std::fs::write(format!("/proc/{child_pid}/uid_map"), format!("0 {uid} 1\n"))?;
    std::fs::write(format!("/proc/{child_pid}/gid_map"), format!("0 {gid} 1\n"))?;

    Ok(false)
}
