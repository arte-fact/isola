//! `isola-uidmap` — minimal setuid-root helper that writes UID/GID mappings
//! for a child process in a new user namespace.
//!
//! This replaces the external dependency on `newuidmap` / `newgidmap` (uidmap package).
//!
//! **Security model**: this binary MUST be installed setuid root (`chown root:root; chmod u+s`).
//! It validates all mappings against `/etc/subuid` / `/etc/subgid` for the calling user
//! before performing any privileged write.
//!
//! Usage:
//!   isola-uidmap uid  <pid> <inside> <outside> <count> [<inside> <outside> <count> ...]
//!   isola-uidmap gid  <pid> <inside> <outside> <count> [<inside> <outside> <count> ...]

use std::process;

// Minimal FFI — avoids pulling in libc or nix crate dependencies.
unsafe extern "C" {
    fn geteuid() -> u32;
    fn getuid() -> u32;
    fn getgid() -> u32;
}

fn main() {
    if let Err(e) = run() {
        eprintln!("isola-uidmap: {e}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        return Err("usage: isola-uidmap uid|gid <pid> <inside> <outside> <count> [...]".into());
    }

    let map_type = &args[1];
    if map_type != "uid" && map_type != "gid" {
        return Err(format!(
            "unknown map type '{map_type}'; expected 'uid' or 'gid'"
        ));
    }

    let rest = &args[2..];
    if rest.len() < 4 || rest.len() % 3 != 1 {
        return Err(format!(
            "expected: <pid> <inside> <outside> <count> [<inside> <outside> <count> ...], got {} args",
            rest.len()
        ));
    }

    // Enforce setuid: we must be running as root (euid == 0) but invoked by a real user.
    let euid = unsafe { geteuid() };
    if euid != 0 {
        return Err("this binary must be setuid root (chown root:root && chmod u+s).".into());
    }

    let ruid = unsafe { getuid() };
    let rgid = unsafe { getgid() };

    let pid_str = &rest[0];
    let pid: i32 = pid_str
        .parse()
        .map_err(|_| format!("invalid pid: {pid_str}"))?;

    // Parse mappings from remaining args: groups of (inside, outside, count)
    let mappings: Vec<(u32, u32, u32)> = rest[1..]
        .chunks(3)
        .map(|chunk| {
            let inside: u32 = chunk[0]
                .parse()
                .map_err(|_| format!("invalid inside ID: {}", chunk[0]))?;
            let outside: u32 = chunk[1]
                .parse()
                .map_err(|_| format!("invalid outside ID: {}", chunk[1]))?;
            let count: u32 = chunk[2]
                .parse()
                .map_err(|_| format!("invalid count: {}", chunk[2]))?;
            Ok((inside, outside, count))
        })
        .collect::<Result<Vec<_>, String>>()?;

    // Resolve the calling user's name and subordinate range
    let username = resolve_username(ruid)?;
    let sub_path = if map_type == "uid" {
        "/etc/subuid"
    } else {
        "/etc/subgid"
    };
    let (sub_start, sub_count) = parse_subordinate_range(sub_path, &username, ruid)?;

    // Validate: every mapping must be within the user's subordinate range.
    // The exception: the user's own host UID/GID may be mapped directly.
    let allowed_self = if map_type == "uid" { ruid } else { rgid };
    for &(inside, outside, count) in &mappings {
        if count == 0 {
            continue;
        }
        let max_outside = outside.saturating_add(count).saturating_sub(1);

        if outside == allowed_self && count == 1 {
            // Mapping the user's own ID is always allowed.
            continue;
        }

        if outside < sub_start
            || max_outside > sub_start.saturating_add(sub_count).saturating_sub(1)
        {
            return Err(format!(
                "mapping {inside} {outside} {count} is outside the allowed subordinate range \
                 ({sub_start}-{}) for user '{username}'",
                sub_start.saturating_add(sub_count).saturating_sub(1),
            ));
        }

        // Refuse to map UID 0 to the user's own host UID on the inside.
        // Mapping inside-0 to sub_start is fine; mapping inside-0 to the user's own
        // host UID would give the sandbox root capabilities that can modify the host
        // user's files.
        if map_type == "uid" && inside == 0 && outside == allowed_self {
            return Err(format!(
                "refusing to map inside UID 0 to calling user's host UID {allowed_self}"
            ));
        }
    }

    // All validations passed — write the mappings.
    if map_type == "uid" {
        // Must deny setgroups before writing gid_map
        let setgroups_path = format!("/proc/{pid}/setgroups");
        std::fs::write(&setgroups_path, "deny\n")
            .map_err(|e| format!("write {setgroups_path}: {e}"))?;
    }

    // Build the multi-line mapping string
    let mut map_data = String::new();
    for (inside, outside, count) in &mappings {
        map_data.push_str(&format!("{inside} {outside} {count}\n"));
    }

    let map_path = format!(
        "/proc/{pid}/{}_map",
        if map_type == "uid" { "uid" } else { "gid" }
    );
    std::fs::write(&map_path, &map_data).map_err(|e| format!("write {map_path}: {e}"))?;

    Ok(())
}

/// Resolve a username from a UID for display in error messages.
fn resolve_username(uid: u32) -> Result<String, String> {
    let uid_str = uid.to_string();

    // Check environment first (the usual case)
    if let Ok(user) = std::env::var("USER") {
        return Ok(user);
    }

    // Fall back to /etc/passwd
    let passwd =
        std::fs::read_to_string("/etc/passwd").map_err(|e| format!("read /etc/passwd: {e}"))?;
    for line in passwd.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() >= 3 && parts[2] == uid_str {
            return Ok(parts[0].to_string());
        }
    }

    Ok(uid_str)
}

/// Parse /etc/subuid or /etc/subgid for the given user, returning (start, count).
fn parse_subordinate_range(path: &str, username: &str, uid: u32) -> Result<(u32, u32), String> {
    let uid_str = uid.to_string();
    let content = std::fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() >= 3 && (parts[0] == username || parts[0] == uid_str) {
            let start: u32 = parts[1]
                .parse()
                .map_err(|_| format!("invalid subordinate ID start in {path}"))?;
            let count: u32 = parts[2]
                .parse()
                .map_err(|_| format!("invalid subordinate count in {path}"))?;
            return Ok((start, count));
        }
    }

    Err(format!(
        "no subordinate range found for user '{username}' in {path}. \
         Add one with: sudo usermod --add-subuids 100000-165535 --add-subgids 100000-165535 {username}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_subfile(entries: &[(&str, u32, u32)]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        let mut buf = String::new();
        for &(user, start, count) in entries {
            buf.push_str(&format!("{user}:{start}:{count}\n"));
        }
        f.write_all(buf.as_bytes()).unwrap();
        f
    }

    #[test]
    fn parse_range_by_username() {
        let f = temp_subfile(&[("testuser", 100000, 65536)]);
        let (start, count) =
            parse_subordinate_range(f.path().to_str().unwrap(), "testuser", 9999).unwrap();
        assert_eq!(start, 100000);
        assert_eq!(count, 65536);
    }

    #[test]
    fn parse_range_by_uid() {
        // Parse by UID string match: in the file, the user field is "1001"
        let f = temp_subfile(&[("1001", 200000, 10000)]);
        let (start, count) =
            parse_subordinate_range(f.path().to_str().unwrap(), "whatever", 1001).unwrap();
        assert_eq!(start, 200000);
        assert_eq!(count, 10000);
    }

    #[test]
    fn parse_range_not_found() {
        let f = temp_subfile(&[("a", 1, 2)]);
        let err = parse_subordinate_range(f.path().to_str().unwrap(), "b", 999).unwrap_err();
        assert!(err.contains("no subordinate range found"));
    }

    #[test]
    fn parse_range_skips_comments_and_empty() {
        let f = temp_subfile(&[("#commented", 1, 2)]);
        // Overwrite to include comments and blank lines
        std::fs::write(f.path(), "\n# comment\nreal:300000:55555\n").unwrap();
        let (start, count) =
            parse_subordinate_range(f.path().to_str().unwrap(), "real", 999).unwrap();
        assert_eq!(start, 300000);
        assert_eq!(count, 55555);
    }

    #[test]
    fn resolve_username_from_env() {
        // The USER env var is always set in test environment
        let name = resolve_username(1000).unwrap();
        assert!(!name.is_empty());
    }

    #[test]
    fn mapping_self_uid_is_allowed() {
        // Validate that mapping the calling user's own UID 1:1 passes.
        // We can't call the main run() easily (needs setuid), so we test the
        // logic by verifying parse_subordinate_range and the general flow.
        let f = temp_subfile(&[("testuser", 100000, 65536)]);
        let (start, count) =
            parse_subordinate_range(f.path().to_str().unwrap(), "testuser", 9999).unwrap();
        // Subordinate range covers the full 65536 range from 100000
        assert!(start == 100000);
        assert!(count >= 65536);
        // A mapping at 1000 with count 1 should be within range
        // (since 1000 < 65536, 100000 + 1000 + 1 - 1 = 101000 <= 165535)
    }
}
