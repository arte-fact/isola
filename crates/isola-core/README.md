# isola-core

The engine behind [isola](https://github.com/arte-fact/isola), as a reusable
library. Create persistent, isolated Linux sandboxes (user namespaces; a Lima VM
on macOS) from your own Rust program — provision them with toolchains, run
processes confined inside them, and tear them down. No CLI, no UI, no prompting.

Use it to **box in** untrusted or semi-trusted work — AI coding agents, build
steps, plugin code — so it can't touch the host. Your program manages the
sandbox; whatever you run inside finds itself as an unprivileged user in an
Ubuntu userland with only a workspace directory shared back to the host.

## Add the dependency

```toml
[dependencies]
isola-core = { git = "https://github.com/arte-fact/isola", tag = "v1.1.0", package = "isola-core" }
```

(Publish to crates.io later for `isola-core = "1"`.)

## Quick start

```rust
use isola_core::{NoProgress, Sandbox, SandboxSpec};

fn run_task() -> Result<(), isola_core::IsolaError> {
    let spec = SandboxSpec {
        workspace: Some("/runs/task-42".into()), // bind-mounted into the sandbox
        plugins: vec!["python-uv".into(), "git".into()],
        ..SandboxSpec::new("task-42")
    };

    let sb = Sandbox::create(&spec, &NoProgress)?; // download base + provision

    // Run something confined inside. Stdio is inherited (redirect it as you like);
    // returns the process exit code.
    let code = sb.exec(&["python".into(), "agent.py".into()], None, vec![])?;

    // Results land in the workspace bind-mount → already on the host filesystem.
    sb.destroy()?;
    Ok(())
}
```

## The API

### `SandboxSpec` — what to build

```rust
pub struct SandboxSpec {
    pub name: String,                      // sandbox id + dir under ~/.isola/sandboxes/
    pub workspace: Option<PathBuf>,        // host dir bind-mounted at /<dirname> (default: cwd)
    pub plugins: Vec<String>,              // toolchains / host imports / shell to install
    pub shell: SandboxShell,               // default: bash
    pub share_display: bool,               // X11/Wayland passthrough (off by default)
    pub no_cache: bool,                    // provision from scratch
    pub plugin_vars: BTreeMap<String, String>, // answers to plugin prompts (e.g. PHP_VERSION)
}

SandboxSpec::new("name")                   // sensible defaults; fill the rest with `..`
```

List installable plugins with the registry, or see
[docs/plugins.md](https://github.com/arte-fact/isola/blob/main/docs/plugins.md):

```rust
use isola_core::plugin::{PluginLayer, PluginRegistry};

let reg = PluginRegistry::load()?;
for p in reg.plugins_for_layer(PluginLayer::Project) {
    println!("{}: {}", p.manifest.name, p.manifest.description);
}
```

### `Sandbox` — manage one

```rust
Sandbox::create(&spec, &progress) -> Result<Sandbox>   // create + provision
Sandbox::open(name) -> Sandbox                         // handle to an existing one
sb.exec(&cmd, workspace_override, devices) -> Result<i32>  // run confined, get exit code
sb.is_healthy() -> bool
sb.name() -> &str
sb.destroy() -> Result<()>                             // delete the rootfs
```

`exec` runs the command as the unprivileged `sandbox` user (uid 1000), inside the
namespaces + seccomp filter — it cannot see host processes or files outside the
workspace. `workspace_override` is usually `None` (uses the configured workspace);
`devices` is usually `vec![]` (extra `/dev` nodes to pass through, e.g. for GPUs).

### Progress

`Sandbox::create` takes `&dyn ProgressReporter`. Pass [`NoProgress`] to ignore it,
or implement the trait to surface download/provision progress in your own UI:

```rust
struct MyProgress;
impl isola_core::ProgressReporter for MyProgress {
    fn start_step(&self, msg: &str) { log::info!("{msg}"); }
    fn provision_phase(&self, n: usize, total: usize, name: &str) { /* … */ }
    // all methods have no-op defaults — override what you care about
}
```

## Platform & host setup

- **Linux (x86_64)** — user namespaces. On Ubuntu 24.04+, AppArmor blocks
  unprivileged user namespaces by default. The **library never prompts or runs
  `sudo`**; it reports this as a `preflight`/create error. Prepare the host once,
  at deploy time, and the library just runs.
- **macOS** — a Lima VM (`brew install lima`).

The `host` module is pure detection + profile generation:

```rust
use isola_core::host;

if !host::host_requirements().is_empty() {
    // e.g. AppArmorUsernsRestricted — install a profile for THIS binary:
    let profile = host::apparmor_profile_for("/usr/local/bin/my-orchestrator");
    // write it to /etc/apparmor.d/ and reload (a privileged deploy step)
}
```

Note the AppArmor profile is keyed to the **executable that creates the sandbox**
— so it's *your* binary's path, not isola's. The simplest deployments avoid all
of this by **running as root or inside a container/VM**, where unprivileged-userns
restrictions don't apply.

## Important: what's shared

- **The workspace** is bind-mounted read-write — that's how results flow back to
  the host. Files created inside are owned by your host user.
- **The network is shared** with the host (no network namespace). For genuinely
  untrusted code, run your orchestrator inside a network-isolated VM/container.

Full threat model:
[docs/security.md](https://github.com/arte-fact/isola/blob/main/docs/security.md).

## License

Apache-2.0.
