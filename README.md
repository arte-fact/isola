# isola

Persistent, isolated Linux sandboxes for running [Claude Code](https://docs.anthropic.com/en/docs/claude-code). Each sandbox is a lightweight container built with Linux user namespaces — no Docker or root privileges required.

## How it works

`isola` downloads an Ubuntu 24.04 base rootfs, provisions it with your chosen development tools, and enters it via `clone()` with `CLONE_NEWUSER | CLONE_NEWPID | CLONE_NEWNS`. Your host project directory is bind-mounted at `/workspace` inside the sandbox. Claude Code runs as a non-root `sandbox` user with `--dangerously-skip-permissions`, fully isolated from your host system.

Sandboxes are persistent — installed packages, config files, and everything outside `/workspace` survive across sessions.

```
┌─ Host ──────────────────────────────────────────────┐
│                                                     │
│  ~/my-project/  ◄──bind-mount──►  /workspace        │
│                                                     │
│  ~/.isola/                                          │
│    cache/            downloaded rootfs tarball       │
│    session/          shared Claude credentials      │
│    sandboxes/                                       │
│      my-project/                                    │
│        config.json   sandbox metadata               │
│        rootfs/       Ubuntu 24.04 filesystem        │
│                                                     │
│  ┌─ Sandbox (user/PID/mount namespaces) ──────────┐ │
│  │  PID 1: bash or claude                         │ │
│  │  UID 1000 (sandbox) → mapped to host UID       │ │
│  │  /workspace ← host project (read-write)        │ │
│  │  /proc, /sys, /dev ← isolated mounts          │ │
│  │  Network: shared with host (no net namespace)  │ │
│  └────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────┘
```

## Requirements

- Linux (x86_64)
- Rust 2024 edition toolchain (to build)
- `newuidmap` / `newgidmap` (optional — install with `sudo apt install uidmap` for multi-UID mapping; without it the sandbox uses a single-UID fallback)
- Claude Code binary on `$PATH` or in `~/.local/bin/claude` (optional — falls back to a shell)

## Installation

```
cargo install --path .
isola setup-host     # Ubuntu: install AppArmor profile for user namespace support
```

## Quick start

Run `isola` in any project directory to launch the interactive setup wizard:

```
cd ~/my-project
isola
```

The wizard will prompt you to:
1. Name the sandbox (defaults to the directory name)
2. Select development environments to install (Rust, Node.js, Python+uv, Go)
3. Confirm the workspace directory to mount
4. Optionally import your host Claude session and settings

Once created, the sandbox launches Claude Code automatically. On subsequent runs, `isola` detects the existing sandbox and enters it directly.

## Usage

```
isola                              # Auto-detect sandbox or run setup wizard
isola create <name> [-w <path>]    # Create a new sandbox
isola enter <name> [--shell]       # Enter sandbox (Claude Code or shell)
isola shell [<name>]               # Open a root shell (auto-detects sandbox from cwd)
isola exec <name> -- <cmd...>      # Run a command inside a sandbox
isola status <name>                # Show sandbox status
isola reprovision <name>           # Re-run provisioning scripts
isola list                         # List all sandboxes
isola destroy <name>               # Delete a sandbox and its rootfs
isola completions <shell>          # Generate shell completions (bash, zsh, fish, etc.)
isola setup-host                   # Install AppArmor profile (Ubuntu, one-time)
```

## Environments

During setup you can choose which toolchains to provision (none selected by default):

| Environment   | What gets installed                          |
|---------------|----------------------------------------------|
| `rust`        | rustup + cargo                               |
| `nodejs`      | Node.js 22 LTS + npm                        |
| `python-uv`  | Python 3 + [uv](https://github.com/astral-sh/uv) |
| `go`          | Latest Go release                            |

Base packages (git, curl, build-essential, ripgrep, fd, bat, etc.) are always installed.

Each selected environment also adds language-specific best practices to the sandbox's `CLAUDE.md`, so Claude Code automatically follows idiomatic conventions (e.g. `cargo clippy` for Rust, `uv run` for Python, `gofmt` for Go).

## Sandbox layout

```
~/.isola/
  cache/                         # Downloaded rootfs tarballs
  session/                       # Shared Claude credentials across sandboxes
  sandboxes/<name>/
    config.json                  # Sandbox metadata (name, workspace, environments)
    rootfs/                      # Ubuntu 24.04 root filesystem
```

Inside the sandbox:

- `/workspace` — bind-mounted from your host project directory (read-write)
- `/home/sandbox` — persistent home directory for the `sandbox` user
- Network access is shared with the host (no network namespace)
- PID and mount namespaces are isolated

## How sandbox isolation works

`isola` uses three Linux namespaces:

- **User namespace** — with `uidmap` installed, maps your host UID to the `sandbox` user (UID 1000) via `newuidmap`/`newgidmap` with a full subordinate range. Without `uidmap`, falls back to a single-UID mapping where everything runs as UID 0 inside (mapped to your host UID)
- **PID namespace** — processes inside the sandbox cannot see or signal host processes
- **Mount namespace** — the sandbox has its own filesystem tree via `pivot_root`; only the workspace directory is shared

No setuid binaries, no daemon, no container runtime. Just `clone()` + `pivot_root()` + `execve()`.

## Claude Code integration

- The host Claude binary is automatically detected and bind-mounted into the sandbox
- Claude Code runs with `--dangerously-skip-permissions` (safe since the sandbox is isolated)
- Sandbox permissions are pre-configured to allow all Claude Code tools
- Session credentials can be imported from the host during setup and are shared across sandboxes via `~/.isola/session/`
- Host git identity is automatically inherited
- `ANTHROPIC_API_KEY` and cloud provider env vars (AWS, Bedrock, Vertex) are forwarded into the sandbox

## License

See [LICENSE](LICENSE) for details.
