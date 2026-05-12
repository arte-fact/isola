use std::os::fd::AsRawFd;
use std::path::PathBuf;

use nix::libc;

use crate::error::IsolaError;

/// Find the bundled `isola-uidmap` setuid helper, or fall back to system
/// `newuidmap`/`newgidmap` (from the `uidmap` package).
pub fn find_uidmap_helper() -> Option<PathBuf> {
    let binary_name = "isola-uidmap";

    // 1. Look next to the current executable
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let candidate = dir.join(binary_name);
        if candidate.exists() {
            return Some(candidate);
        }
    }

    // 2. Check PATH
    if let Ok(output) = std::process::Command::new("which")
        .arg(binary_name)
        .output()
        && output.status.success()
    {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let p = PathBuf::from(&path);
        if p.exists() {
            return Some(p);
        }
    }

    None
}

/// Check whether a usable uidmap helper is available.
/// Tries our bundled `isola-uidmap` first, then falls back to system tools.
pub fn has_uidmap_tools() -> bool {
    // Prefer our bundled helper
    if find_uidmap_helper().is_some() {
        return true;
    }

    // Fall back to system newuidmap/newgidmap
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
        eprintln!(
            "Warning: uidmap helper not found (install with `isola setup-host`), falling back to single-UID mapping"
        );
    }

    std::fs::write(format!("/proc/{child_pid}/setgroups"), "deny\n")?;
    std::fs::write(format!("/proc/{child_pid}/uid_map"), format!("0 {uid} 1\n"))?;
    std::fs::write(format!("/proc/{child_pid}/gid_map"), format!("0 {gid} 1\n"))?;

    Ok(false)
}

/// Write multi-UID mappings for a child process, falling back to single-UID on failure.
/// Used for best-effort operations like cleanup where failure is acceptable.
pub fn write_id_mappings_best_effort(child_pid: i32) {
    // Try multi-UID first; on any failure fall back to single-UID mapping
    if write_id_mappings(child_pid, true).is_err() {
        let _ = write_id_mappings(child_pid, false);
    }
}

/// Run the uidmap helper (bundled `isola-uidmap` or system `newuidmap`/`newgidmap`).
fn run_uidmap_helper(args: &[String]) -> Result<(), IsolaError> {
    let uidmap_path = find_uidmap_helper();

    if let Some(ref helper_path) = uidmap_path {
        // Use our bundled helper: `isola-uidmap uid|gid <pid> <args...>`
        let status = std::process::Command::new(helper_path)
            .args(args)
            .status()
            .map_err(|e| IsolaError::NamespaceError(format!("Failed to run isola-uidmap: {e}")))?;
        if !status.success() {
            return Err(IsolaError::NamespaceError(
                "isola-uidmap failed. Check that it is setuid root and /etc/sub[ug]id is configured."
                    .into(),
            ));
        }
        return Ok(());
    }

    // Fall back to system tools: `newuidmap <pid> <args...>` or `newgidmap <pid> <args...>`
    let map_type = &args[0]; // "uid" or "gid"
    let binary = if map_type == "uid" {
        "newuidmap"
    } else {
        "newgidmap"
    };

    // System tools expect args without the type prefix: <pid> <inside> <outside> <count> ...
    let system_args: Vec<&str> = args[1..].iter().map(|s| s.as_str()).collect();

    let status = std::process::Command::new(binary)
        .args(&system_args)
        .status()
        .map_err(|e| {
            IsolaError::NamespaceError(format!(
                "Failed to run {binary}: {e}. Install with: sudo apt install uidmap"
            ))
        })?;

    if !status.success() {
        return Err(IsolaError::NamespaceError(format!(
            "{binary} failed. Check /etc/subuid configuration."
        )));
    }

    Ok(())
}

/// Run a closure in a new user namespace. The closure runs as UID 0 (mapped to the
/// caller's host UID). Returns the child's exit code on success.
pub fn run_in_userns<F>(child_fn: F, multi_uid_strict: bool) -> Result<i32, IsolaError>
where
    F: FnOnce() -> i32 + Send + 'static,
{
    let (pipe_read, pipe_write) = nix::unistd::pipe()?;
    let pipe_read_raw = pipe_read.as_raw_fd();
    let pipe_write_raw = pipe_write.as_raw_fd();

    // We need to box the closure and pass it through the clone boundary
    let boxed_fn = Box::new(child_fn);
    let fn_ptr = Box::into_raw(boxed_fn);

    struct CloneArgs {
        pipe_read: i32,
        pipe_write: i32,
        fn_ptr: *mut dyn FnOnce() -> i32,
    }

    // Safety: fn_ptr is only used by the child
    unsafe impl Send for CloneArgs {}

    let mut args = CloneArgs {
        pipe_read: pipe_read_raw,
        pipe_write: pipe_write_raw,
        fn_ptr,
    };

    extern "C" fn child_entry(arg: *mut libc::c_void) -> libc::c_int {
        let args = unsafe { &*(arg as *const CloneArgs) };
        nix::unistd::close(args.pipe_write).ok();
        let mut buf = [0u8; 1];
        nix::unistd::read(args.pipe_read, &mut buf).ok();
        nix::unistd::close(args.pipe_read).ok();

        let boxed_fn = unsafe { Box::from_raw(args.fn_ptr) };
        boxed_fn()
    }

    const STACK_SIZE: usize = 256 * 1024;
    // Safety: ManuallyDrop prevents the stack from being freed while the child
    // process may still be using it. We block on waitpid() before returning.
    let mut stack = std::mem::ManuallyDrop::new(vec![0u8; STACK_SIZE]);
    let flags = libc::CLONE_NEWUSER | libc::SIGCHLD;

    // Safety: clone() creates a new process that uses `stack` and reads `args`.
    // Both outlive the child: we block on waitpid() before this function returns,
    // and `stack` is ManuallyDrop so it won't be freed.
    let child_pid = unsafe {
        libc::clone(
            child_entry,
            stack.as_mut_ptr().add(STACK_SIZE) as *mut libc::c_void,
            flags,
            &mut args as *mut CloneArgs as *mut libc::c_void,
        )
    };

    if child_pid < 0 {
        // Reclaim the closure to avoid leak
        unsafe { drop(Box::from_raw(fn_ptr)) };
        return Err(IsolaError::NamespaceError(format!(
            "clone() failed: {}",
            std::io::Error::last_os_error()
        )));
    }

    drop(pipe_read);

    if multi_uid_strict {
        let _ = write_id_mappings(child_pid, true)?;
    } else {
        write_id_mappings_best_effort(child_pid);
    }

    drop(pipe_write);

    let status = nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(child_pid), None)?;

    match status {
        nix::sys::wait::WaitStatus::Exited(_, code) => Ok(code),
        _ => Ok(1),
    }
}
