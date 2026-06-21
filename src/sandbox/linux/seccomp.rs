//! Minimal seccomp-BPF denylist for the unprivileged sandbox workload.
//!
//! Blocks a small set of capability-gated syscalls that no normal development,
//! build, or provisioning workflow uses but that are prime kernel attack
//! surface / namespace-escape vectors (Docker's default profile blocks these
//! too). It is a *denylist*: everything else keeps working — including
//! `ptrace` (gdb/strace) and the `setuid` path used by in-sandbox `sudo`.
//!
//! The filter is installed while the child still holds CAP_SYS_ADMIN in its
//! user namespace (before dropping to the sandbox user). Because of that,
//! `PR_SET_NO_NEW_PRIVS` is deliberately NOT set — which is what preserves
//! in-sandbox `sudo`. The filter survives the subsequent setuid and execve.

use nix::libc;

use crate::error::IsolaError;

// Classic-BPF opcodes and stable seccomp ABI constants. Several of these are
// not exposed by the `libc` crate, so they are defined here (kernel ABI, fixed).
const BPF_LD: u16 = 0x00;
const BPF_JMP: u16 = 0x05;
const BPF_RET: u16 = 0x06;
const BPF_W: u16 = 0x00;
const BPF_ABS: u16 = 0x20;
const BPF_JEQ: u16 = 0x10;
const BPF_K: u16 = 0x00;

const AUDIT_ARCH_X86_64: u32 = 0xC000_003E;
const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;
const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
const SECCOMP_MODE_FILTER: libc::c_ulong = 2;

// Offsets into `struct seccomp_data { u32 nr; u32 arch; u64 ip; u64 args[6]; }`.
const OFF_NR: u32 = 0;
const OFF_ARCH: u32 = 4;

fn stmt(code: u16, k: u32) -> libc::sock_filter {
    libc::sock_filter {
        code,
        jt: 0,
        jf: 0,
        k,
    }
}

fn jump(code: u16, k: u32, jt: u8, jf: u8) -> libc::sock_filter {
    libc::sock_filter { code, jt, jf, k }
}

/// Capability-gated syscalls blocked for workload sessions. None are used by
/// normal dev/build/provisioning flows; `ptrace`/`perf_event_open` are
/// intentionally NOT blocked so debuggers and profilers keep working.
fn blocked_syscalls() -> [libc::c_long; 10] {
    [
        libc::SYS_bpf,
        libc::SYS_keyctl,
        libc::SYS_add_key,
        libc::SYS_request_key,
        libc::SYS_init_module,
        libc::SYS_finit_module,
        libc::SYS_delete_module,
        libc::SYS_kexec_load,
        libc::SYS_kexec_file_load,
        libc::SYS_open_by_handle_at,
    ]
}

/// Install the seccomp denylist on the calling thread. Requires CAP_SYS_ADMIN
/// in the current user namespace (held by the namespace creator before it drops
/// privileges), so it does not need — and does not set — `no_new_privs`.
pub fn apply_denylist() -> Result<(), IsolaError> {
    let blocked = blocked_syscalls();
    let n = blocked.len();
    let errno_ret = SECCOMP_RET_ERRNO | (libc::EPERM as u32);

    // Program layout:
    //   [0]      load arch
    //   [1]      if arch == x86_64 -> skip kill, else fall through
    //   [2]      RET kill            (blocks 32-bit/x32 syscall-number bypass)
    //   [3]      load syscall nr
    //   [4..4+n] one JEQ per blocked syscall -> jump to ERRNO return
    //   [4+n]    RET allow
    //   [5+n]    RET errno
    let mut prog: Vec<libc::sock_filter> = Vec::with_capacity(n + 6);
    prog.push(stmt(BPF_LD | BPF_W | BPF_ABS, OFF_ARCH));
    prog.push(jump(BPF_JMP | BPF_JEQ | BPF_K, AUDIT_ARCH_X86_64, 1, 0));
    prog.push(stmt(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS));
    prog.push(stmt(BPF_LD | BPF_W | BPF_ABS, OFF_NR));
    for (i, nr) in blocked.iter().enumerate() {
        // ERRNO return lives at index 5 + n; this JEQ is at index 4 + i.
        let jt = (n - i) as u8;
        prog.push(jump(BPF_JMP | BPF_JEQ | BPF_K, *nr as u32, jt, 0));
    }
    prog.push(stmt(BPF_RET | BPF_K, SECCOMP_RET_ALLOW));
    prog.push(stmt(BPF_RET | BPF_K, errno_ret));

    let fprog = libc::sock_fprog {
        len: prog.len() as u16,
        filter: prog.as_mut_ptr(),
    };

    let rc = unsafe {
        libc::prctl(
            libc::PR_SET_SECCOMP,
            SECCOMP_MODE_FILTER,
            &fprog as *const libc::sock_fprog as libc::c_ulong,
            0 as libc::c_ulong,
            0 as libc::c_ulong,
        )
    };
    if rc != 0 {
        return Err(IsolaError::NamespaceError(format!(
            "failed to install seccomp filter: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(())
}
