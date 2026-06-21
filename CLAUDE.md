# isola

Persistent, isolated developer sandboxes. Linux user namespaces on Linux; a Lima
VM on macOS. See `README.md` and `docs/` for the user-facing docs.

## Build & Test

```
cargo build --workspace                       # debug build
cargo build --workspace --release             # release build
cargo test --workspace                        # run all tests
cargo clippy --workspace --all-targets -- -D warnings   # lint (must be clean, no #[allow])
cargo fmt                                      # format
cargo install --path crates/isola-cli         # install the `isola` binary to ~/.cargo/bin
```

Run `cargo fmt` and `cargo clippy --workspace --all-targets -- -D warnings` before committing.
The codebase is kept clippy-clean with **no bare `#[allow]`** — resolve warnings
at the cause (e.g. `#[cfg(test)]` for test-only items, `#[cfg(target_os = …)]`
for platform-specific code), not by suppressing them.

## Architecture

Single-binary CLI (`isola`). Platform behavior is behind a `SandboxBackend`
trait (`src/sandbox/backend.rs`): `LinuxBackend` (namespaces) and `LimaBackend`
(macOS VM). Most trait methods used only by the macOS path are
`#[cfg(target_os = "macos")]`.

- `src/sandbox/`
  - `backend.rs` — `SandboxBackend` trait + `create_backend()`
  - `config.rs` — `SandboxConfig` (per-sandbox JSON), `LocalConfig`
    (`.isola/config.yaml`), `SandboxShell` (name-backed; behavior resolved from
    the shell plugin's manifest)
  - `rootfs/` — rootfs lifecycle, split by concern:
    - `download.rs` — fetch / SHA-verify / extract the Ubuntu base (Linux)
    - `provision.rs` — build the provisioning script + `post_setup_rootfs`
    - `cache.rs` — monolithic + layered provision caches (Linux)
    - `claude_md.rs` — CLAUDE.md generation (macOS/Lima only)
  - `linux/`
    - `namespace.rs` — `clone()` with `NEWUSER|NEWPID|NEWNS|NEWUTS|NEWIPC`, `pivot_root`, `execve`
    - `mounts.rs` — mount setup (proc/sys/dev/tmp, workspace, devices, caches, display)
    - `userns.rs` — UID/GID mapping via `newuidmap`/`newgidmap`, single-UID fallback
    - `seccomp.rs` — syscall denylist (raw classic BPF)
    - `cleanup.rs` — remove subordinate-UID-owned paths via a user namespace
  - `lima/` — macOS backend (Lima VM + template)
- `src/plugin.rs` — `PluginManifest`/`PluginRegistry`; bundled + `~/.isola/plugins` + `<project>/.isola/plugins`
- `src/commands/` — one module per subcommand: `default` (auto-detect/wizard),
  `setup` (wizard → writes `.isola/config.yaml`), `create`, `enter`, `exec`,
  `list`, `status`, `reprovision`, `destroy`, `plugins`, `cache`, `setup_host`
- `src/sha256.rs` — inline streaming SHA-256 (no crypto crate)
- `src/cli.rs` (clap) · `src/error.rs` (`IsolaError`, thiserror) · `src/paths.rs`
- `src/bin/isola-uidmap.rs` — bundled setuid uid/gid map helper

## Key concepts

- Sandboxes live at `~/.isola/sandboxes/<name>/` (a `rootfs/` + `config.json`).
- The workspace is bind-mounted at `/<dirname>` (named after the host project
  dir), read-write; changes are reflected on the host immediately.
- Provisioning runs as in-namespace root; sessions enter as uid 1000 (`sandbox`).
- Everything selectable is a plugin (toolchains, host-config imports, shells).
  The wizard menus and `SandboxShell` behavior are driven by the plugin registry.
- Three caches under `~/.isola/cache/`: base download, provisioned layers, and
  shared package-manager pools. See `docs/caching.md`.

## Code conventions

- Rust 2024 edition; `thiserror` for errors, propagate with `?`.
- `nix` crate for syscalls (not raw libc where possible).
- No external crypto crate — SHA-256 lives in `src/sha256.rs`.
- Tests are co-located in each module behind `#[cfg(test)]`.
- Platform-specific code is `#[cfg(target_os = …)]`; wholly-Linux modules are
  gated once at their `mod` declaration.
