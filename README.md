# isola

Persistent, isolated sandboxes for running [Claude Code](https://docs.anthropic.com/en/docs/claude-code). On Linux, each sandbox is a lightweight container built with user namespaces — no Docker or root privileges required. On macOS, sandboxes run inside lightweight [Lima](https://lima-vm.io/) VMs.

## How it works

### Linux

`isola` downloads an Ubuntu 24.04 base rootfs, provisions it with your chosen development tools, and enters it via `clone()` with `CLONE_NEWUSER | CLONE_NEWPID | CLONE_NEWNS`. Your host project directory is bind-mounted at `/workspace` inside the sandbox. Claude Code runs as a non-root `sandbox` user with `--dangerously-skip-permissions`, fully isolated from your host system.

### macOS

`isola` creates an Ubuntu 24.04 VM using Lima (Apple's Virtualization.framework). Your host project directory is shared at `/workspace` via VirtioFS. The VM is provisioned with the same development tools and runs Claude Code inside the Linux guest.

Sandboxes are persistent on both platforms — installed packages, config files, and everything outside `/workspace` survive across sessions.

## Requirements

### Linux
- Linux (x86_64)
- `newuidmap` / `newgidmap` (optional — install with `sudo apt install uidmap` for multi-UID mapping; without it the sandbox uses a single-UID fallback)
- Rust 2024 edition toolchain (to build)
- Claude Code binary on `$PATH` or in `~/.local/bin/claude` (optional — falls back to a shell)

### macOS
- macOS 13+ (Ventura or later)
- [Lima](https://lima-vm.io/) (`brew install lima`)
- Rust 2024 edition toolchain (to build)
- `ANTHROPIC_API_KEY` environment variable set (passed into the VM automatically)

## Installation

```
cargo install --path .
```

### Linux post-install

```
isola setup-host     # Ubuntu: install AppArmor profile for user namespace support
```

### macOS post-install

```
brew install lima     # if not already installed
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

Once created, the sandbox launches Claude Code automatically.

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
isola destroy <name>               # Delete a sandbox and its rootfs/VM
isola completions <shell>          # Generate shell completions (bash, zsh, fish, etc.)
isola setup-host                   # Install AppArmor profile (Linux/Ubuntu, one-time)
```

## Environments

During setup you can choose which toolchains to provision (none selected by default):

| Environment   | What gets installed                          |
|---------------|----------------------------------------------|
| `rust`        | rustup + cargo                               |
| `nodejs`      | Node.js 22 LTS + npm                        |
| `python-uv`  | Python 3 + [uv](https://github.com/astral-sh/uv) |
| `go`          | Latest Go release                            |

Base packages (git, curl, build-essential, etc.) are always installed.

Each selected environment also adds language-specific best practices to the sandbox's `CLAUDE.md`, so Claude Code automatically follows idiomatic conventions (e.g. `cargo clippy` for Rust, `uv run` for Python, `gofmt` for Go).

## Sandbox layout

```
~/.isola/
  cache/                         # Downloaded rootfs tarballs (Linux)
  sandboxes/<name>/
    config.json                  # Sandbox metadata (name, workspace, environments)
    rootfs/                      # Ubuntu 24.04 root filesystem (Linux)
    lima.yaml                    # Lima VM configuration (macOS)
```

Inside the sandbox:

- `/workspace` — mounted from your host project directory (read-write)
- `/home/sandbox` — persistent home directory for the `sandbox` user
- Network access is shared with the host

## How sandbox isolation works

### Linux

`isola` uses three Linux namespaces:

- **User namespace** — with `uidmap` installed, maps your host UID to the `sandbox` user (UID 1000) via `newuidmap`/`newgidmap` with a full subordinate range. Without `uidmap`, falls back to a single-UID mapping where everything runs as UID 0 inside (mapped to your host UID)
- **PID namespace** — processes inside the sandbox cannot see or signal host processes
- **Mount namespace** — the sandbox has its own filesystem tree via `pivot_root`; only the workspace directory is shared

No setuid binaries, no daemon, no container runtime. Just `clone()` + `pivot_root()` + `execve()`.

### macOS

`isola` delegates to Lima, which uses Apple's Virtualization.framework to run a lightweight Linux VM:

- **VirtioFS** shares the workspace directory between host and guest
- The VM runs Ubuntu 24.04 with the same provisioning as the Linux backend
- Claude Code is installed inside the VM via npm
- `ANTHROPIC_API_KEY` and other environment variables are forwarded into the VM

## License

See [LICENSE](LICENSE) for details.
