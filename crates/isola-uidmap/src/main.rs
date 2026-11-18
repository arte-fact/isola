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

use std::fs::File;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
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
    if pid <= 0 {
        return Err(format!("invalid pid: {pid_str}"));
    }

    // The target must be a process of the calling user. Keep its /proc
    // directory open and write through that handle, so the check and the
    // privileged writes are guaranteed to refer to the same process.
    let proc_dir = open_owned_proc_dir(Path::new(&format!("/proc/{pid}")), ruid)?;
    let proc_fd = proc_dir.as_raw_fd();

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

    let allowed_self = if map_type == "uid" { ruid } else { rgid };
    validate_mappings(
        map_type == "uid",
        &mappings,
        (sub_start, sub_count),
        allowed_self,
    )
    .map_err(|e| format!("{e} (user '{username}')"))?;

    // All validations passed — write the mappings.
    if map_type == "uid" {
        // Must deny setgroups before writing gid_map
        let setgroups_path = format!("/proc/self/fd/{proc_fd}/setgroups");
        std::fs::write(&setgroups_path, "deny\n")
            .map_err(|e| format!("write {setgroups_path}: {e}"))?;
    }

    // Build the multi-line mapping string
    let mut map_data = String::new();
    for (inside, outside, count) in &mappings {
        map_data.push_str(&format!("{inside} {outside} {count}\n"));
    }

    let map_path = format!(
        "/proc/self/fd/{proc_fd}/{}_map",
        if map_type == "uid" { "uid" } else { "gid" }
    );
    std::fs::write(&map_path, &map_data).map_err(|e| format!("write {map_path}: {e}"))?;

    Ok(())
}

/// Check every `(inside, outside, count)` mapping against what the caller may
/// map: IDs within their subordinate range `(start, count)`, plus their own
/// host ID (`allowed_self`) as a single entry.
///
/// For UID maps, inside UID 0 may never be the caller's own host UID: sandbox
/// root would then act on the host as the calling user. That rule is checked
/// first, before the "own ID is always allowed" shortcut can accept the entry.
fn validate_mappings(
    is_uid_map: bool,
    mappings: &[(u32, u32, u32)],
    (sub_start, sub_count): (u32, u32),
    allowed_self: u32,
) -> Result<(), String> {
    let sub_end = sub_start.saturating_add(sub_count).saturating_sub(1);
    for &(inside, outside, count) in mappings {
        if count == 0 {
            continue;
        }

        if is_uid_map && inside == 0 && outside == allowed_self {
            return Err(format!(
                "refusing to map inside UID 0 to calling user's host UID {allowed_self}"
            ));
        }

        if outside == allowed_self && count == 1 {
            // Mapping the user's own ID (to a non-root inside ID) is allowed.
            continue;
        }

        let max_outside = outside.saturating_add(count).saturating_sub(1);
        if sub_count == 0 || outside < sub_start || max_outside > sub_end {
            return Err(format!(
                "mapping {inside} {outside} {count} is outside the allowed subordinate range \
                 ({sub_start}-{sub_end})"
            ));
        }
    }
    Ok(())
}

/// Open a process's `/proc/<pid>` directory, refusing it unless the process
/// belongs to `expected_uid` (the caller's real UID).
///
/// Without this check any user could ask the setuid helper to write the ID
/// maps of someone else's process. Ownership is read from the opened handle
/// (fstat), not from the path, so the PID cannot be swapped in between.
fn open_owned_proc_dir(proc_dir: &Path, expected_uid: u32) -> Result<File, String> {
    let dir = File::open(proc_dir).map_err(|e| format!("open {}: {e}", proc_dir.display()))?;
    let meta = dir
        .metadata()
        .map_err(|e| format!("stat {}: {e}", proc_dir.display()))?;
    if !meta.is_dir() {
        return Err(format!("{} is not a directory", proc_dir.display()));
    }
    if meta.uid() != expected_uid {
        return Err(format!(
            "{} belongs to UID {}, not to the calling user (UID {expected_uid}); \
             refusing to write its ID maps",
            proc_dir.display(),
            meta.uid()
        ));
    }
    Ok(dir)
}

/// Resolve the calling user's name from their **real UID**.
///
/// This name selects which `/etc/subuid` / `/etc/subgid` entry authorises the
/// mappings, so it must never come from the environment: this binary is setuid
/// root and `USER` is controlled by the caller, who could otherwise claim
/// another user's subordinate range.
fn resolve_username(uid: u32) -> Result<String, String> {
    let passwd =
        std::fs::read_to_string("/etc/passwd").map_err(|e| format!("read /etc/passwd: {e}"))?;
    // Users absent from /etc/passwd (e.g. NSS/LDAP accounts) fall back to the
    // numeric UID, which subuid/subgid files also accept as the owner field.
    Ok(username_from_passwd(&passwd, uid).unwrap_or_else(|| uid.to_string()))
}

/// Find the login name owning `uid` in passwd-formatted `content`.
fn username_from_passwd(content: &str, uid: u32) -> Option<String> {
    let uid_str = uid.to_string();
    content.lines().find_map(|line| {
        let mut fields = line.split(':');
        let name = fields.next()?;
        let _password = fields.next()?;
        (fields.next()? == uid_str && !name.is_empty()).then(|| name.to_string())
    })
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

    const PASSWD: &str = "root:x:0:0:root:/root:/bin/bash\n\
                          alice:x:1000:1000::/home/alice:/bin/bash\n\
                          bob:x:1001:1001::/home/bob:/bin/bash\n";

    #[test]
    fn username_is_looked_up_by_uid() {
        assert_eq!(username_from_passwd(PASSWD, 1001).as_deref(), Some("bob"));
        assert_eq!(username_from_passwd(PASSWD, 0).as_deref(), Some("root"));
        assert_eq!(username_from_passwd(PASSWD, 4242), None);
    }

    #[test]
    fn username_lookup_matches_the_uid_field_only() {
        // A GID or a UID prefix must not match.
        let passwd = "carol:x:2000:1001::/home/carol:/bin/sh\nd:x:10010:5::/:/bin/sh\n";
        assert_eq!(username_from_passwd(passwd, 1001), None);
    }

    #[test]
    fn resolve_username_ignores_the_environment() {
        // Run the lookup in a child process whose USER/LOGNAME name someone
        // else: the result must still be derived from the real UID.
        if std::env::var("ISOLA_UIDMAP_ENV_PROBE").is_ok() {
            let uid = unsafe { getuid() };
            let expected = std::fs::read_to_string("/etc/passwd")
                .ok()
                .and_then(|p| username_from_passwd(&p, uid))
                .unwrap_or_else(|| uid.to_string());
            assert_eq!(resolve_username(uid).unwrap(), expected);
            assert_ne!(resolve_username(uid).unwrap(), "someone-else");
            return;
        }
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tests::resolve_username_ignores_the_environment"])
            .env("ISOLA_UIDMAP_ENV_PROBE", "1")
            .env("USER", "someone-else")
            .env("LOGNAME", "someone-else")
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn own_process_is_accepted() {
        let uid = unsafe { getuid() };
        let dir = open_owned_proc_dir(Path::new("/proc/self"), uid).unwrap();
        assert!(dir.metadata().unwrap().is_dir());
    }

    #[test]
    fn process_of_another_user_is_refused() {
        let uid = unsafe { getuid() };
        // Our own process, checked against a UID that is not ours.
        let err = open_owned_proc_dir(Path::new("/proc/self"), uid.wrapping_add(1)).unwrap_err();
        assert!(err.contains("not to the calling user"), "{err}");

        // PID 1 belongs to root: refused for any unprivileged caller.
        if uid != 0 {
            let err = open_owned_proc_dir(Path::new("/proc/1"), uid).unwrap_err();
            assert!(err.contains("belongs to UID 0"), "{err}");
        }
    }

    #[test]
    fn missing_process_and_non_directory_are_refused() {
        let uid = unsafe { getuid() };
        let err = open_owned_proc_dir(Path::new("/proc/0"), uid).unwrap_err();
        assert!(err.starts_with("open /proc/0"), "{err}");

        let f = tempfile::NamedTempFile::new().unwrap();
        let err = open_owned_proc_dir(f.path(), uid).unwrap_err();
        assert!(err.contains("is not a directory"), "{err}");
    }

    const SUB: (u32, u32) = (100_000, 65_536);
    const SELF_ID: u32 = 1000;

    #[test]
    fn isola_multi_uid_layout_is_allowed() {
        // The exact layout isola-core requests: root and system IDs from the
        // subordinate range, the sandbox user (1000) mapped to the host user.
        let maps = [
            (0, 100_000, 1),
            (1, 100_001, 999),
            (1000, SELF_ID, 1),
            (1001, 101_000, 64_535),
        ];
        assert_eq!(validate_mappings(true, &maps, SUB, SELF_ID), Ok(()));
        assert_eq!(validate_mappings(false, &maps, SUB, SELF_ID), Ok(()));
    }

    #[test]
    fn inside_root_to_own_host_uid_is_refused() {
        let err = validate_mappings(true, &[(0, SELF_ID, 1)], SUB, SELF_ID).unwrap_err();
        assert!(err.contains("refusing to map inside UID 0"), "{err}");

        // Still refused when hidden among otherwise valid entries.
        let maps = [(1, 100_001, 999), (0, SELF_ID, 1)];
        assert!(validate_mappings(true, &maps, SUB, SELF_ID).is_err());
    }

    #[test]
    fn inside_root_to_own_host_gid_is_allowed() {
        // The UID 0 rule is specific to UID maps.
        assert_eq!(
            validate_mappings(false, &[(0, SELF_ID, 1)], SUB, SELF_ID),
            Ok(())
        );
    }

    #[test]
    fn ids_outside_the_subordinate_range_are_refused() {
        for maps in [
            [(0, 0, 1)],              // host root
            [(0, 99_999, 1)],         // just below the range
            [(0, 165_535, 2)],        // runs past the end
            [(5, SELF_ID, 2)],        // own ID, but more than one
            [(0, u32::MAX, 5)],       // overflowing range
            [(1000, SELF_ID + 1, 1)], // someone else's host ID
        ] {
            let err = validate_mappings(true, &maps, SUB, SELF_ID).unwrap_err();
            assert!(
                err.contains("outside the allowed subordinate range"),
                "{err}"
            );
        }
    }

    #[test]
    fn empty_subordinate_range_allows_only_own_id() {
        assert_eq!(
            validate_mappings(true, &[(1000, SELF_ID, 1)], (0, 0), SELF_ID),
            Ok(())
        );
        assert!(validate_mappings(true, &[(0, 0, 1)], (0, 0), SELF_ID).is_err());
    }
}
