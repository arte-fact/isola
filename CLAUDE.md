# isola

Persistent, isolated Linux sandboxes for developers using user namespaces.

## Build & Test

```
cargo build                        # debug build
cargo build --release              # release build
cargo test                         # run all tests
cargo test <module>::tests         # run tests for a specific module
cargo clippy -- -D warnings        # lint
cargo fmt                          # format
cargo install --path .             # install to ~/.cargo/bin
```

Run `cargo fmt` and `cargo clippy` before committing.

## Architecture

Single-binary CLI (`isola`) with two main module trees:

- `src/sandbox/` — low-level Linux namespace and filesystem operations
  - `namespace.rs` — `clone()` with `CLONE_NEWUSER|NEWPID|NEWNS`, `pivot_root()`, `execve()`
  - `mounts.rs` — mount setup (proc, sys, dev, tmp, workspace bind-mount, SSH)
  - `userns.rs` — UID/GID mapping via `newuidmap`/`newgidmap` with single-UID fallback
  - `rootfs.rs` — Ubuntu 24.04 rootfs download/caching, SHA256 verification, provisioning scripts
  - `config.rs` — `SandboxConfig` JSON serialization (name, workspace, environments, timestamps)

- `src/commands/` — CLI subcommand implementations
  - `default.rs` — auto-detect sandbox from cwd or launch interactive wizard
  - `setup.rs` — interactive wizard (name, environments, workspace prompts via `inquire`)
  - `create.rs` — create sandbox: validate name, download rootfs, extract, provision
  - `enter.rs` — enter sandbox: build env vars, `enter_sandbox()`
  - `exec.rs` — run arbitrary command inside sandbox as UID 1000
  - `destroy.rs` — delete sandbox (chown via userns, then `remove_dir_all`)
  - `list.rs` / `status.rs` / `reprovision.rs` / `setup_host.rs`

- `src/cli.rs` — clap CLI definition
- `src/error.rs` — `IsolaError` enum (thiserror)
- `src/paths.rs` — path helpers (`~/.isola/sandboxes/`, `~/.isola/cache/`, etc.)

## Key concepts

- Sandboxes live at `~/.isola/sandboxes/<name>/` with a `rootfs/` directory and `config.json`
- Rootfs tarball is downloaded once and cached at `~/.isola/cache/`
- Provisioning runs as root inside a user namespace, then sandbox enters as UID 1000 (`sandbox` user)
- Workspace is bind-mounted from host at `/workspace`; changes are immediately reflected on host

## Code conventions

- Rust 2024 edition
- `thiserror` for all error types; propagate with `?`
- `nix` crate for syscalls (not raw libc where possible)
- No external crypto crate — SHA256 is implemented inline in `rootfs.rs`
- Tests are co-located in each module behind `#[cfg(test)]`
