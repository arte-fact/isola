# isola

Persistent, isolated sandboxes for developers. Each sandbox is a lightweight container built with Linux user namespaces (on Linux) or a lightweight VM via Lima (on macOS). No Docker or root privileges required.

## How it works

`isola` provisions an Ubuntu 24.04 environment with your chosen development tools, fully isolated from your host system.

**On Linux**, it uses `clone()` with `CLONE_NEWUSER | CLONE_NEWPID | CLONE_NEWNS` to create a namespace-based sandbox. Your host project directory is bind-mounted at `/workspace`.

**On macOS**, it creates a lightweight Linux VM using [Lima](https://lima-vm.io/) with Apple's Virtualization.framework. Your project directory is shared via VirtioFS at `/workspace`.

Sandboxes are persistent: installed packages, config files, and everything outside `/workspace` survive across sessions.

```
┌─ Host ──────────────────────────────────────────────┐
│                                                     │
│  ~/my-project/  ◄──bind-mount──►  /workspace        │
│                                                     │
│  ~/.isola/                                          │
│    cache/            downloaded rootfs tarball       │
│    sandboxes/                                       │
│      my-project/                                    │
│        config.json   sandbox metadata               │
│        rootfs/       Ubuntu 24.04 filesystem        │
│                                                     │
│  ┌─ Sandbox (namespace or VM) ────────────────────┐ │
│  │  PID 1: <your shell>                           │ │
│  │  UID 1000 (sandbox) → mapped to host UID       │ │
│  │  /workspace ← host project (read-write)        │ │
│  │  /proc, /sys, /dev ← isolated mounts          │ │
│  │  Network: shared with host                     │ │
│  └────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────┘
```

## Requirements

### Linux (x86_64)
- Rust 2024 edition toolchain (to build)
- `newuidmap` / `newgidmap` (optional, install with `sudo apt install uidmap` for multi-UID mapping; without it the sandbox uses a single-UID fallback)

### macOS (Apple Silicon or Intel)
- [Lima](https://lima-vm.io/) (install with `brew install lima`)
- Rust 2024 edition toolchain (to build)

## Installation

```
cargo install --path .
```

On Linux (Ubuntu with AppArmor):
```
isola setup-host     # Install AppArmor profile for user namespace support
```

On macOS, ensure Lima is installed:
```
brew install lima
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
4. Choose your shell (bash, fish, zsh)

Once created, the sandbox launches your shell automatically. On subsequent runs, `isola` detects the existing sandbox and enters it directly.

## Usage

```
isola                              # Enter the sandbox for the current directory (or run the setup wizard)
isola create <name> [-w <path>]    # Create a new sandbox
isola enter <name>                 # Enter a sandbox shell by name, from anywhere
isola exec <name> -- <cmd...>      # Run a command inside a sandbox
isola status <name>                # Show sandbox status
isola reprovision <name>           # Re-run provisioning scripts
isola list                         # List all sandboxes
isola destroy <name>               # Delete a sandbox
isola completions <shell>          # Generate shell completions (bash, zsh, fish, etc.)
isola setup-host                   # Install AppArmor profile (Linux/Ubuntu only, one-time)
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

## Sandbox layout

```
~/.isola/
  cache/                         # Downloaded rootfs tarballs (Linux only)
  sandboxes/<name>/
    config.json                  # Sandbox metadata (name, workspace, backend, environments)
    rootfs/                      # Ubuntu 24.04 root filesystem (Linux)
    lima.yaml                    # Lima VM configuration (macOS)
```

Inside the sandbox:

- `/workspace` — mounted from your host project directory (read-write)
- `/home/sandbox` — persistent home directory for the `sandbox` user
- Network access is shared with the host
- On Linux: PID and mount namespaces are isolated
- On macOS: full VM isolation via Apple Virtualization.framework

## How sandbox isolation works

### Linux

`isola` uses three Linux namespaces:

- **User namespace** — with `uidmap` installed, maps your host UID to the `sandbox` user (UID 1000) via `newuidmap`/`newgidmap` with a full subordinate range. Without `uidmap`, falls back to a single-UID mapping where everything runs as UID 0 inside (mapped to your host UID)
- **PID namespace** — processes inside the sandbox cannot see or signal host processes
- **Mount namespace** — the sandbox has its own filesystem tree via `pivot_root`; only the workspace directory is shared

No setuid binaries, no daemon, no container runtime. Just `clone()` + `pivot_root()` + `execve()`.

### macOS

`isola` uses [Lima](https://lima-vm.io/) to run a lightweight Linux VM:

- **Virtualization.framework** (`vmType: vz`) for near-native performance on Apple Silicon
- **VirtioFS** for fast, coherent file sharing between host and VM
- **Rosetta** support for running x86_64 binaries on ARM Macs

Each sandbox is a dedicated Lima VM instance. The VM is started on demand when you enter the sandbox and persists between sessions.

## Team sharing

The setup wizard saves a `.isola/config.yaml` in your project directory. Commit this file so teammates can create identical sandboxes by running `isola` in the project.

## License

See [LICENSE](LICENSE) for details.
