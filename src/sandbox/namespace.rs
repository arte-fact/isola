use std::ffi::CString;
use std::mem::ManuallyDrop;
use std::os::fd::AsRawFd;
use std::path::Path;

use nix::libc;

use crate::error::IsolaError;
use crate::sandbox::mounts;
use crate::sandbox::userns;

fn to_cstring(s: &str, label: &str) -> Result<CString, IsolaError> {
    CString::new(s).map_err(|_| IsolaError::NamespaceError(format!("{label} contains a null byte")))
}

/// What to execute inside the sandbox
pub struct SandboxExec {
    pub rootfs: String,
    pub exec_path: String,
    pub exec_args: Vec<String>,
    pub env_vars: Vec<String>,
    pub workspace_host: Option<String>,
    pub claude_binary: Option<String>,
    pub session_credentials: Option<String>,
    pub run_as_uid: Option<u32>,
    pub multi_uid: bool,
}

struct ChildArgs {
    sync_pipe_read: i32,
    sync_pipe_write: i32,
    rootfs: CString,
    exec_path: CString,
    exec_args: Vec<CString>,
    env_vars: Vec<CString>,
    workspace_host: Option<CString>,
    claude_binary: Option<CString>,
    session_credentials: Option<CString>,
    run_as_uid: Option<u32>,
}

pub fn enter_sandbox(exec: SandboxExec) -> Result<i32, IsolaError> {
    let child_args = ChildArgs {
        sync_pipe_read: -1,
        sync_pipe_write: -1,
        rootfs: to_cstring(&exec.rootfs, "rootfs path")?,
        exec_path: to_cstring(&exec.exec_path, "exec path")?,
        exec_args: exec
            .exec_args
            .iter()
            .map(|s| to_cstring(s, "exec arg"))
            .collect::<Result<Vec<_>, _>>()?,
        env_vars: exec
            .env_vars
            .iter()
            .map(|s| to_cstring(s, "env var"))
            .collect::<Result<Vec<_>, _>>()?,
        workspace_host: exec
            .workspace_host
            .as_ref()
            .map(|s| to_cstring(s, "workspace path"))
            .transpose()?,
        claude_binary: exec
            .claude_binary
            .as_ref()
            .map(|s| to_cstring(s, "claude binary path"))
            .transpose()?,
        session_credentials: exec
            .session_credentials
            .as_ref()
            .map(|s| to_cstring(s, "session credentials path"))
            .transpose()?,
        run_as_uid: exec.run_as_uid,
    };

    // Create sync pipe
    let (pipe_read, pipe_write) = nix::unistd::pipe()?;

    let pipe_read_raw = pipe_read.as_raw_fd();
    let pipe_write_raw = pipe_write.as_raw_fd();

    const STACK_SIZE: usize = 1024 * 1024;
    // Safety: ManuallyDrop prevents the stack from being freed while the child
    // process may still be using it. We intentionally leak this allocation;
    // the child runs on this stack until execve() replaces the process image.
    let mut stack = ManuallyDrop::new(vec![0u8; STACK_SIZE]);

    let mut packed_args = ChildArgs {
        sync_pipe_read: pipe_read_raw,
        sync_pipe_write: pipe_write_raw,
        ..child_args
    };

    let flags = libc::CLONE_NEWUSER | libc::CLONE_NEWPID | libc::CLONE_NEWNS | libc::SIGCHLD;

    // Safety: clone() creates a new process that uses `stack` and reads `packed_args`.
    // Both outlive the child: we block on waitpid() before this function returns,
    // and `stack` is ManuallyDrop so it won't be freed.
    let child_pid = unsafe {
        libc::clone(
            child_entry,
            stack.as_mut_ptr().add(STACK_SIZE) as *mut libc::c_void,
            flags,
            &mut packed_args as *mut ChildArgs as *mut libc::c_void,
        )
    };

    if child_pid < 0 {
        return Err(IsolaError::NamespaceError(format!(
            "clone() failed: {}",
            std::io::Error::last_os_error()
        )));
    }

    // Parent: close read end
    drop(pipe_read);

    // Write UID/GID mappings via shared helper
    let got_multi_uid = userns::write_id_mappings(child_pid, exec.multi_uid)?;

    // Signal child: 1 = multi-UID active, 0 = single-UID fallback
    nix::unistd::write(&pipe_write, &[if got_multi_uid { 1u8 } else { 0u8 }])?;
    drop(pipe_write);

    // Wait for child
    let status = nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(child_pid), None)?;

    match status {
        nix::sys::wait::WaitStatus::Exited(_, code) => Ok(code),
        _ => Ok(1),
    }
}

extern "C" fn child_entry(arg: *mut libc::c_void) -> libc::c_int {
    let args = unsafe { &*(arg as *const ChildArgs) };
    match child_main(args) {
        Ok(_) => 0,
        Err(e) => {
            eprintln!("sandbox error: {e}");
            1
        }
    }
}

fn child_main(args: &ChildArgs) -> Result<(), IsolaError> {
    // Close our copy of the write end first — otherwise read() blocks forever
    nix::unistd::close(args.sync_pipe_write).ok();

    // Wait for parent to write uid_map/gid_map.
    // The byte value signals: 1 = multi-UID active, 0 = single-UID fallback.
    let mut buf = [0u8; 1];
    let n = nix::unistd::read(args.sync_pipe_read, &mut buf).unwrap_or(0);
    nix::unistd::close(args.sync_pipe_read).ok();
    let got_multi_uid = n == 1 && buf[0] == 1;

    let rootfs_str = args
        .rootfs
        .to_str()
        .map_err(|_| IsolaError::NamespaceError("rootfs path is not valid UTF-8".into()))?;
    let rootfs = Path::new(rootfs_str);

    // Set up mounts
    mounts::setup_mounts(
        rootfs,
        args.workspace_host
            .as_ref()
            .and_then(|s| s.to_str().ok())
            .map(Path::new),
        args.claude_binary
            .as_ref()
            .and_then(|s| s.to_str().ok())
            .map(Path::new),
        args.session_credentials
            .as_ref()
            .and_then(|s| s.to_str().ok())
            .map(Path::new),
    )?;

    // pivot_root
    do_pivot_root(rootfs)?;

    // Drop to non-root user if requested (only possible with multi-UID mapping)
    if let Some(uid) = args.run_as_uid
        && got_multi_uid
    {
        nix::unistd::setgid(nix::unistd::Gid::from_raw(uid))
            .map_err(|e| IsolaError::NamespaceError(format!("setgid({uid}) failed: {e}")))?;
        nix::unistd::setuid(nix::unistd::Uid::from_raw(uid))
            .map_err(|e| IsolaError::NamespaceError(format!("setuid({uid}) failed: {e}")))?;
    }

    // Start in /workspace if it exists
    let _ = nix::unistd::chdir("/workspace");

    // Exec
    let args_refs: Vec<&std::ffi::CStr> = args.exec_args.iter().map(|s| s.as_c_str()).collect();
    let env_refs: Vec<&std::ffi::CStr> = args.env_vars.iter().map(|s| s.as_c_str()).collect();

    nix::unistd::execve(&args.exec_path, &args_refs, &env_refs)?;
    unreachable!()
}

fn do_pivot_root(rootfs: &Path) -> Result<(), IsolaError> {
    nix::unistd::chdir(rootfs)?;

    let old_root = rootfs.join(".old_root");
    std::fs::create_dir_all(&old_root)?;

    nix::unistd::pivot_root(".", ".old_root")?;
    nix::unistd::chdir("/")?;

    nix::mount::umount2("/.old_root", nix::mount::MntFlags::MNT_DETACH)?;
    std::fs::remove_dir("/.old_root")?;

    Ok(())
}
