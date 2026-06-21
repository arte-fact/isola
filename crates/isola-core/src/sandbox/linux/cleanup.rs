//! Delete host paths that may contain files owned by mapped subordinate UIDs.
//!
//! The shared apt cache is written by provisioning, which runs as in-namespace
//! root → its files (and the dir apt chowns to root) land owned by the user's
//! subordinate UID range (e.g. 100000) on the host, which the unprivileged host
//! user cannot remove directly. This spawns a user namespace with the full
//! multi-UID mapping; the namespace creator holds CAP_DAC_OVERRIDE over every
//! mapped UID, so it can `remove_dir_all` those paths. No mount/pivot is needed.

use std::mem::ManuallyDrop;
use std::os::fd::AsRawFd;
use std::path::PathBuf;

use nix::libc;

use super::userns;
use crate::error::IsolaError;

struct Args {
    sync_read: i32,
    sync_write: i32,
    paths: Vec<PathBuf>,
}

extern "C" fn child_entry(arg: *mut libc::c_void) -> libc::c_int {
    let args = unsafe { &*(arg as *const Args) };
    nix::unistd::close(args.sync_write).ok();
    // Wait until the parent has written our UID/GID maps.
    let mut buf = [0u8; 1];
    let sync_read = unsafe { std::os::fd::BorrowedFd::borrow_raw(args.sync_read) };
    nix::unistd::read(sync_read, &mut buf).ok();
    nix::unistd::close(args.sync_read).ok();

    // As the namespace creator we hold full capabilities (incl. CAP_DAC_OVERRIDE)
    // over all mapped UIDs, so we can delete subordinate-owned files.
    for p in &args.paths {
        if p.exists() {
            let _ = std::fs::remove_dir_all(p);
        }
    }
    0
}

/// Remove the given host paths from inside a multi-UID user namespace.
pub fn remove_paths(paths: Vec<PathBuf>) -> Result<(), IsolaError> {
    if paths.is_empty() {
        return Ok(());
    }

    let (pipe_read, pipe_write) =
        nix::unistd::pipe().map_err(|e| IsolaError::NamespaceError(format!("pipe(): {e}")))?;
    let mut args = Args {
        sync_read: pipe_read.as_raw_fd(),
        sync_write: pipe_write.as_raw_fd(),
        paths,
    };

    const STACK_SIZE: usize = 256 * 1024;
    let mut stack = ManuallyDrop::new(vec![0u8; STACK_SIZE]);
    let flags = libc::CLONE_NEWUSER | libc::SIGCHLD;

    // Safety: the child uses `stack` and `args`; we block on waitpid() before
    // returning, and `stack` is ManuallyDrop so it outlives the child.
    let child_pid = unsafe {
        libc::clone(
            child_entry,
            stack.as_mut_ptr().add(STACK_SIZE) as *mut libc::c_void,
            flags,
            &mut args as *mut Args as *mut libc::c_void,
        )
    };
    if child_pid < 0 {
        return Err(IsolaError::NamespaceError(format!(
            "clone() failed: {}",
            std::io::Error::last_os_error()
        )));
    }

    drop(pipe_read);
    // Full multi-UID mapping so the child can reach the subordinate range; fall
    // back to single-UID, which still clears most (host-user-owned) cache files.
    let _ = userns::write_id_mappings(child_pid, true);
    nix::unistd::write(&pipe_write, &[1u8])
        .map_err(|e| IsolaError::NamespaceError(format!("sync write: {e}")))?;
    drop(pipe_write);

    nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(child_pid), None)
        .map_err(|e| IsolaError::NamespaceError(format!("waitpid: {e}")))?;
    Ok(())
}
