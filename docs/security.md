# Security & isolation model

isola is built to **contain blast radius**, not to be an impenetrable security
boundary. It's a great fit for keeping AI agents, untrusted scripts, and messy
builds away from your home directory and host system. It is *not* a substitute
for a VM or a hardened container when running genuinely malicious code. This page
is explicit about where the lines are.

## What is isolated

On **Linux**, each sandbox runs in its own set of kernel namespaces, created with
`clone()`:

- **User namespace** — the sandbox's `root` is not the host's root. Your shell
  runs as the unprivileged `sandbox` user (uid 1000).
- **Mount namespace** — the sandbox has its own filesystem tree (`pivot_root`
  into the sandbox rootfs). It cannot see your host filesystem except the one
  directory you mount as the workspace.
- **PID namespace** — the sandbox cannot see or signal host processes. Its shell
  is PID 1 inside.
- **UTS + IPC namespaces** — its own hostname and System V IPC / POSIX message
  queues.

On top of that:

- **Filesystem** — the sandbox lives in `~/.isola/sandboxes/<name>/rootfs/`,
  a self-contained Ubuntu userland. Nothing it installs touches your host.
- **seccomp filter** — for interactive/exec sessions, a syscall **denylist** is
  installed that blocks a set of rarely-needed, high-risk calls: `bpf`,
  `keyctl`/`add_key`/`request_key`, kernel module loading
  (`init_module`/`finit_module`/`delete_module`), `kexec_*`, and
  `open_by_handle_at`. This is installed while still holding `CAP_SYS_ADMIN` so
  that **`sudo` keeps working inside the sandbox** — `no_new_privs` is *not* set,
  which is a deliberate usability trade-off.

On **macOS**, the sandbox is a full Lima VM, which is a stronger boundary than
namespaces (it's a separate kernel).

## What is shared (by design)

These are intentional, for convenience — know them before you trust a sandbox:

- **Network.** There is **no network namespace**. The sandbox shares the host's
  network and has full, unrestricted internet access. It can reach `localhost`
  services on your host and your LAN.
- **The workspace.** Your project directory is bind-mounted read-write at
  `/<dirname>`. Code in the sandbox can read and modify those files — that's the
  point. Files created there are owned by your host user.
- **Anything a plugin imports.** `host_mount` entries (e.g. `ssh-keys`,
  `git-config`) bind-mount parts of your host `$HOME` into the sandbox.
  Read-only mounts (like SSH keys) can still be *read* by sandbox code. Only add
  imports you're comfortable exposing.
- **The display, if you opt in.** Choosing "share host display" bind-mounts your
  X11/Wayland socket so GUI apps work — but that socket lets sandbox code observe
  input and capture the screen. It defaults to off-ish and the wizard warns you.
- **GPU devices, if you select a GPU plugin** — passed through as `/dev` nodes.

## UID mapping

With `uidmap` installed (`isola setup-host` arranges this), isola maps a full
range of subordinate UIDs: the in-sandbox `root` (uid 0) maps to an unprivileged
host subuid, and the `sandbox` user (uid 1000) maps back to **your** host user.
That's why files you create in the workspace are owned by you, and why in-sandbox
`sudo`/`apt` work without affecting the host.

Without `uidmap`, isola falls back to a single-UID mapping (host user → sandbox
root). It still works, but it's coarser; the multi-UID range is recommended.

## Threat model — be honest about it

**Good for:**
- Stopping an AI agent or a script from trashing your home dir, dotfiles, or
  system packages.
- Reproducible, disposable per-project toolchains.
- Running build steps and dependencies you don't fully trust *with your data*.

**Not sufficient for:**
- Running code you believe is actively malicious. A user namespace shares the
  host kernel; a kernel-level exploit can escape. Use a real VM (the macOS path,
  or your own VM) for that.
- **Network-based** containment. There is no network isolation — sandboxed code
  can exfiltrate data or attack your LAN. If you need this, run isola inside a
  network-isolated VM.
- Protecting secrets you've explicitly imported. If you mount your SSH keys, the
  sandbox can use them.

## Hardening checklist

- Run `isola setup-host` so you get the full multi-UID mapping.
- Only enable host imports (`ssh-keys`, `git-config`, …) you actually need.
- Don't share the display unless you need GUI apps.
- For genuinely untrusted code, run isola **inside a VM** to add a network and
  kernel boundary.
