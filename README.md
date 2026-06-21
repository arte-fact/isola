<div align="center">

# isola

**Persistent, isolated dev sandboxes — no Docker, no root.**

Give every project its own throwaway Ubuntu environment. Install whatever you
want inside it; your host stays clean. Your code lives on the host and is
mounted live into the sandbox, so edits flow both ways instantly.

</div>

```bash
cd ~/code/my-api
isola                 # wizard → pick tools → drops you in a sandbox shell
# inside: install packages, run builds, let an AI agent loose — host is untouched
```

---

## Why isola?

- **No root, no daemon.** Built on Linux *user namespaces* (and a Lima VM on
  macOS). Nothing runs as root on your host; there's no background service.
- **Your tools, isolated.** Each sandbox is a full Ubuntu 24.04 userland. `sudo
  apt install` anything inside — it never touches your machine.
- **Live workspace.** Your project directory is bind-mounted into the sandbox.
  Edit on the host in your editor, build inside the sandbox; changes are the
  same files, instantly.
- **Persistent.** Installed packages and config survive across sessions. Re-enter
  a sandbox days later and it's exactly as you left it.
- **Fast to recreate.** Aggressive caching means rebuilding the *same* sandbox is
  near-instant, and package downloads are shared across all your sandboxes.
- **Safe for agents.** A natural blast-radius limit for AI coding agents and
  untrusted scripts: they can scribble all over the sandbox, not your home dir.

## Install

```bash
cargo install --path crates/isola-cli          # builds and installs the `isola` binary
```

**Linux (x86_64)** — uses user namespaces, no privileges needed at runtime.

On Ubuntu 24.04+, AppArmor blocks unprivileged user namespaces by default. The
**first time** you create a sandbox, isola detects this and offers to run a
one-time host setup for you (installing an AppArmor profile and subordinate UID
ranges) — it uses `sudo`, so you just confirm once. You can also run it
explicitly at any time:

```bash
isola setup-host                # installs an AppArmor profile + subordinate UID range
```

This isn't done fully silently because it modifies system files (`/etc/apparmor.d`,
`/etc/subuid`) via `sudo` — isola asks before touching your host. For best
isolation it also sets up `uidmap`-style multi-UID mapping; without it, isola
falls back to a coarser single-UID mapping that still works.

**macOS (Apple Silicon / Intel)** — uses a [Lima](https://lima-vm.io/) VM:

```bash
brew install lima
cargo install --path crates/isola-cli
```

## Quick start

```bash
cd ~/code/my-project

isola                           # no sandbox yet → setup wizard, then enter
```

The wizard asks four things — shell, host config to import, tools to install,
and any tool versions — then writes a small `.isola/config.yaml`, creates the
sandbox, and drops you into a shell. You land in `/my-project`, your live
workspace.

From then on:

```bash
cd ~/code/my-project && isola   # re-enters the existing sandbox instantly
isola enter my-project          # ...or enter it by name from anywhere
isola exec my-project -- cargo test     # run one command, non-interactively
```

Prefer no wizard? Create explicitly:

```bash
isola create my-project -w ~/code/my-project --plugins rust,nodejs
```

## How it works

`isola` runs your shell inside a fresh Ubuntu 24.04 userland that's isolated from
your host with kernel namespaces (Linux) or a lightweight VM (macOS).

```
┌─ Host ─────────────────────────────────────────────────────┐
│                                                            │
│  ~/code/my-project/  ◄──── bind-mount ────►  /my-project   │  ← live, two-way
│                                                            │
│  ~/.isola/                                                 │
│    cache/        shared rootfs + package downloads          │
│    sandboxes/my-project/                                   │
│      config.json    metadata (tools, shell, workspace)      │
│      rootfs/        the sandbox's Ubuntu filesystem         │
│                                                            │
│  ┌─ Sandbox ──────────────────────────────────────────┐   │
│  │  your shell as user `sandbox` (uid 1000)            │   │
│  │  own PID / mount / UTS / IPC namespaces             │   │
│  │  /my-project ← your project (read-write)            │   │
│  │  sudo works inside; never touches the host          │   │
│  │  network: shared with host                          │   │
│  └─────────────────────────────────────────────────────┘   │
└────────────────────────────────────────────────────────────┘
```

Key facts:

- **Your workspace is mounted at `/<dirname>`** — a project at `~/code/my-project`
  appears as `/my-project` inside, on both Linux and macOS. It's the same files
  as on the host.
- **Everything else persists** in `~/.isola/sandboxes/<name>/rootfs/` and survives
  across sessions. Delete it with `isola destroy <name>`.
- **Files you create in the workspace are owned by you** on the host (the
  sandbox's uid 1000 maps back to your user), so there's no `chown` dance.
- **The network is shared** with the host — sandboxes have full internet, and
  there is no network isolation by default.

See **[docs/security.md](docs/security.md)** for exactly what is and isn't
isolated.

## Concepts

| Concept | What it means |
|---|---|
| **Sandbox** | A named, persistent Ubuntu environment at `~/.isola/sandboxes/<name>/`. |
| **Workspace** | Your host project dir, bind-mounted live at `/<dirname>`. Changes are shared both ways. |
| **Plugin** | A unit of capability — a toolchain, a host-config import, or a shell. The wizard menu is built from these. See [docs/plugins.md](docs/plugins.md). |
| **`.isola/config.yaml`** | A small file in your project recording the chosen shell/tools. Commit it to share the setup with your team. |
| **Caches** | Shared, reused downloads that make creation and builds fast. See [docs/caching.md](docs/caching.md). |

## Commands

```
isola                          Enter the sandbox for the current directory,
                               or run the setup wizard if there's no config yet.

isola create <name> [opts]     Create a sandbox.
    -w, --workspace <path>       Host dir to mount (default: current dir).
    --plugins <a,b,c>            Tools to install (default: all project plugins).
    --no-cache                   Force fresh provisioning, ignore caches.

isola enter <name> [opts]      Enter a sandbox shell by name, from anywhere.
    -w, --workspace <path>       Override the mounted workspace.
    --device <path>              Extra host device to pass through (e.g. /dev/dri).

isola exec <name> -- <cmd>     Run one command inside a sandbox and exit.
isola list                     List all sandboxes.
isola status <name>            Show a sandbox's tools, shell, and health.
isola reprovision <name>       Re-run provisioning (e.g. after editing config).
isola destroy <name>           Delete a sandbox and its rootfs.
isola plugins                  List available plugins (bundled, user, project).
isola cache clean [--all]      Clear shared package caches (--all: rootfs caches too).
isola setup-host               One-time Linux host setup (AppArmor + subuid range).
isola completions <shell>      Generate shell completions (bash, zsh, fish, …).
```

## Plugins (the tools you can install)

Everything selectable is a plugin — toolchains, host-config imports, and even
the shells. Run `isola plugins` to see them all.

| Layer | Plugins |
|---|---|
| **Toolchains** | `rust` · `nodejs` · `python-uv` · `go` · `php` |
| **GPU** | `cuda` (NVIDIA) · `rocm` (AMD) · `gpu` (device access) |
| **Tools** | `git` · `neovim` · `fzf` · `chromium` · `claude` (Claude Code) |
| **Host imports** | `git-config` · `ssh-keys` · `claude-config` · `fish-config` · `zsh-config` · `neovim-config` |
| **Shells** | `bash` · `fish` · `zsh` |

Drop your own plugin in `~/.isola/plugins/<name>/` (a `plugin.yaml` + an
`install.sh`) and it appears in the menu automatically.
**[docs/plugins.md](docs/plugins.md)** has the manifest reference and a worked
example.

## Team sharing

The wizard writes a `.isola/config.yaml` in your project:

```yaml
environments: [rust, git-config, ssh-keys]
shell: bash
```

Commit it. When a teammate runs `isola` in that checkout, isola reads the config
and builds the same sandbox automatically — no wizard, no instructions to follow.

## GPU / CUDA

Select the `cuda` (NVIDIA) or `rocm` (AMD) plugin and isola passes the host GPU
device nodes into the sandbox and bind-mounts the matching driver libraries, so
`nvcc` and CUDA/HIP workloads run inside. The wizard flags GPU plugins when no
compatible GPU is detected on the host. Details in
**[docs/caching.md](docs/caching.md)** and the plugin docs.

## Use it as a library

The sandbox engine is a separate crate, **`isola-core`**, so your own Rust
program can create, run-things-in, and destroy sandboxes — handy for boxing in
AI agents, build steps, or untrusted code. The `isola` CLI is just a thin layer
over it.

```toml
[dependencies]
isola-core = { git = "https://github.com/arte-fact/isola", tag = "v1.1.0", package = "isola-core" }
```

```rust
use isola_core::{NoProgress, Sandbox, SandboxSpec};

let spec = SandboxSpec {
    plugins: vec!["python-uv".into(), "git".into()],
    ..SandboxSpec::new("task-42")
};
let sb = Sandbox::create(&spec, &NoProgress)?;          // download + provision
let code = sb.exec(&["python".into(), "agent.py".into()], None, vec![])?;
sb.destroy()?;
```

Full library guide: **[crates/isola-core/README.md](crates/isola-core/README.md)**.

This repo is a Cargo workspace: `crates/isola-core` (library), `crates/isola-cli`
(the `isola` binary), `crates/isola-uidmap` (the setuid helper).

## Documentation

- **[crates/isola-core/README.md](crates/isola-core/README.md)** — using `isola-core` as a library in a Rust project.
- **[docs/plugins.md](docs/plugins.md)** — using and authoring plugins; manifest reference.
- **[docs/security.md](docs/security.md)** — the isolation model: what's isolated, what's shared, the threat model.
- **[docs/caching.md](docs/caching.md)** — how the rootfs, layer, and package caches make things fast.

## Troubleshooting

- **`isola: command not found`** — `~/.cargo/bin` isn't on your `PATH`. Add it, or run the binary by full path.
- **Creation fails on Ubuntu with a namespace/clone error** — AppArmor restricts unprivileged user namespaces on recent Ubuntu. isola offers to fix this on first run; if you declined or ran non-interactively, run `isola setup-host`.
- **A tool isn't found inside the sandbox** — make sure its plugin was installed (`isola status <name>`); re-add it and `isola reprovision <name>`.
- **The shared package cache filled up** — `isola cache clean` (add `--all` to also drop the rootfs caches).
- **`Sandbox '<name>' not found`** — list what exists with `isola list`; names come from the project directory.

## License

Licensed under the [Apache License 2.0](LICENSE).
