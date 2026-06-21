use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use crate::error::IsolaError;

/// Find the bundled `isola-uidmap` setuid-root helper next to the isola binary.
///
/// Returns the path only if it exists **and** is setuid root — a non-setuid
/// helper cannot write multi-line UID maps, so in that case we return `None`
/// and fall back to the system `newuidmap`/`newgidmap` tools rather than letting
/// an unusable bundled helper shadow them.
pub fn find_uidmap_helper() -> Option<PathBuf> {
    let candidate = std::env::current_exe().ok()?.parent()?.join("isola-uidmap");
    let meta = std::fs::metadata(&candidate).ok()?;
    if meta.permissions().mode() & 0o4000 != 0 {
        Some(candidate)
    } else {
        None
    }
}

/// Whether the system `newuidmap`/`newgidmap` tools are both available.
fn system_uidmap_tools() -> bool {
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

/// Whether a usable multi-UID mapping tool is available: either the bundled
/// setuid `isola-uidmap` helper, or the system `newuidmap`/`newgidmap` tools.
pub fn has_uidmap_tools() -> bool {
    find_uidmap_helper().is_some() || system_uidmap_tools()
}

/// Run the uidmap helper for one map type. `args` is `[uid|gid, pid, inside,
/// outside, count, ...]`. Prefers the bundled setuid `isola-uidmap`, falling
/// back to system `newuidmap`/`newgidmap` (which take the same args without the
/// leading type token).
fn run_uidmap_helper(args: &[String]) -> Result<(), IsolaError> {
    if let Some(helper) = find_uidmap_helper() {
        let status = std::process::Command::new(&helper).args(args).status();
        if let Ok(s) = status
            && s.success()
        {
            return Ok(());
        }
        // Bundled helper failed — fall through to system tools if present.
    }

    let map_type = &args[0]; // "uid" or "gid"
    let binary = if map_type == "uid" {
        "newuidmap"
    } else {
        "newgidmap"
    };
    let system_args: Vec<&str> = args[1..].iter().map(|s| s.as_str()).collect();
    let status = std::process::Command::new(binary)
        .args(&system_args)
        .status()
        .map_err(|e| {
            IsolaError::NamespaceError(format!(
                "Failed to run {binary}: {e}. Install with: sudo apt install uidmap, \
                 or run `isola setup-host` to enable the bundled helper."
            ))
        })?;
    if !status.success() {
        return Err(IsolaError::NamespaceError(format!(
            "{binary} failed. Check /etc/sub{map_type} configuration."
        )));
    }
    Ok(())
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
        let uid_args: Vec<String> = vec![
            "uid".into(),
            pid_str.clone(),
            "0".into(),
            sub_uid_start.to_string(),
            "1".into(),
            "1".into(),
            (sub_uid_start + 1).to_string(),
            "999".into(),
            "1000".into(),
            uid.to_string(),
            "1".into(),
            "1001".into(),
            (sub_uid_start + 1000).to_string(),
            uid_remaining.to_string(),
        ];
        run_uidmap_helper(&uid_args)?;

        let gid_remaining = sub_gid_count.saturating_sub(1000).min(64535);
        let gid_args: Vec<String> = vec![
            "gid".into(),
            pid_str.clone(),
            "0".into(),
            sub_gid_start.to_string(),
            "1".into(),
            "1".into(),
            (sub_gid_start + 1).to_string(),
            "999".into(),
            "1000".into(),
            gid.to_string(),
            "1".into(),
            "1001".into(),
            (sub_gid_start + 1000).to_string(),
            gid_remaining.to_string(),
        ];
        run_uidmap_helper(&gid_args)?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parse_valid_entry_by_username() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .unwrap_or_else(|_| "nobody".to_string());
        writeln!(f, "{user}:100000:65536").unwrap();
        let (start, count) = parse_subordinate_ids(f.path().to_str().unwrap()).unwrap();
        assert_eq!(start, 100000);
        assert_eq!(count, 65536);
    }

    #[test]
    fn parse_valid_entry_by_uid() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        let uid = nix::unistd::getuid().as_raw();
        writeln!(f, "{uid}:200000:1000").unwrap();
        let (start, count) = parse_subordinate_ids(f.path().to_str().unwrap()).unwrap();
        assert_eq!(start, 200000);
        assert_eq!(count, 1000);
    }

    #[test]
    fn parse_no_matching_user() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "nonexistentuser:100000:65536").unwrap();
        let result = parse_subordinate_ids(f.path().to_str().unwrap());
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("No subordinate ID range found"));
    }

    #[test]
    fn parse_malformed_count() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .unwrap_or_else(|_| "nobody".to_string());
        writeln!(f, "{user}:100000:notanumber").unwrap();
        let result = parse_subordinate_ids(f.path().to_str().unwrap());
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Invalid subordinate count"));
    }

    #[test]
    fn parse_empty_file() {
        let f = tempfile::NamedTempFile::new().unwrap();
        let result = parse_subordinate_ids(f.path().to_str().unwrap());
        assert!(result.is_err());
    }

    #[test]
    fn parse_multiple_entries_first_match() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .unwrap_or_else(|_| "nobody".to_string());
        writeln!(f, "otheruser:50000:1000").unwrap();
        writeln!(f, "{user}:100000:65536").unwrap();
        writeln!(f, "{user}:200000:10000").unwrap();
        let (start, count) = parse_subordinate_ids(f.path().to_str().unwrap()).unwrap();
        assert_eq!(start, 100000);
        assert_eq!(count, 65536);
    }

    #[test]
    fn parse_missing_file() {
        let result = parse_subordinate_ids("/nonexistent/path/subuid");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Failed to read"));
    }

    #[test]
    fn parse_short_line_skipped() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .unwrap_or_else(|_| "nobody".to_string());
        writeln!(f, "{user}:100000").unwrap(); // Only 2 fields
        writeln!(f, "{user}:200000:500").unwrap();
        let (start, count) = parse_subordinate_ids(f.path().to_str().unwrap()).unwrap();
        assert_eq!(start, 200000);
        assert_eq!(count, 500);
    }
}
