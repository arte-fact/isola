# bot

Persistent, isolated Linux sandboxes for running [Claude Code](https://docs.anthropic.com/en/docs/claude-code). Each sandbox is a lightweight container built with Linux user namespaces — no Docker or root privileges required.

## How it works

`bot` downloads an Ubuntu 24.04 base rootfs, provisions it with your chosen development tools, and enters it via `clone()` with `CLONE_NEWUSER | CLONE_NEWPID | CLONE_NEWNS`. Your host project directory is bind-mounted at `/workspace` inside the sandbox. Claude Code runs as a non-root `sandbox` user with `--dangerously-skip-permissions`, fully isolated from your host system.

Sandboxes are persistent — installed packages, config files, and everything outside `/workspace` survive across sessions.

## Requirements

- Linux (x86_64)
- `newuidmap` / `newgidmap` (install with `sudo apt install uidmap`)
- Rust 2024 edition toolchain (to build)
- Claude Code binary on `$PATH` or in `~/.local/bin/claude` (optional — falls back to a shell)

## Installation

```
cargo install --path .
```

## Quick start

Run `bot` in any project directory to launch the interactive setup wizard:

```
cd ~/my-project
bot
```

The wizard will prompt you to:
1. Name the sandbox (defaults to the directory name)
2. Select development environments to install (Rust, Node.js, Python+uv, Go)
3. Confirm the workspace directory to mount

Once created, the sandbox launches Claude Code automatically.

## Usage

```
bot                              # Auto-detect sandbox or run setup wizard
bot create <name> [-w <path>]    # Create a new sandbox
bot enter <name> [--shell]       # Enter sandbox (Claude Code or shell)
bot shell [<name>]               # Open a root shell (auto-detects sandbox from cwd)
bot exec <name> -- <cmd...>      # Run a command inside a sandbox
bot status <name>                # Show sandbox status
bot reprovision <name>           # Re-run provisioning scripts
bot list                         # List all sandboxes
bot destroy <name>               # Delete a sandbox and its rootfs
bot completions <shell>          # Generate shell completions (bash, zsh, fish, etc.)
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
~/.bot/
  cache/                         # Downloaded rootfs tarballs
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

`bot` uses three Linux namespaces:

- **User namespace** — maps your host UID to root inside the sandbox via `newuidmap`/`newgidmap`, with a subordinate range for the `sandbox` user (UID 1000)
- **PID namespace** — processes inside the sandbox cannot see or signal host processes
- **Mount namespace** — the sandbox has its own filesystem tree via `pivot_root`; only the workspace directory is shared

No setuid binaries, no daemon, no container runtime. Just `clone()` + `pivot_root()` + `execve()`.

## License

See [LICENSE](LICENSE) for details.
