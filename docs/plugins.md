# Plugins

In isola, **everything you can select is a plugin** — language toolchains,
host-config imports, and even the shells. The setup wizard's menus are generated
from the installed plugins, and you can add your own.

```bash
isola plugins        # list every available plugin, its layer, and its source
```

## The three layers

| Layer | Purpose | In the wizard |
|---|---|---|
| **project** | Install software into the sandbox (toolchains, tools, GPU access). | "Project tooling" multi-select. |
| **user** | Import personal config/credentials from your host (read-only where it matters). | "User setup" multi-select, pre-checked when detected on your host. |
| **shell** | A selectable login shell (`bash`, `fish`, `zsh`). | The "Shell" picker. |

## Bundled plugins

| Layer | Plugins |
|---|---|
| Toolchains (project) | `rust`, `nodejs`, `python-uv`, `go`, `php` |
| GPU (project) | `cuda` (NVIDIA toolkit), `rocm` (AMD), `gpu` (device access) |
| Tools (project) | `git`, `neovim`, `fzf`, `chromium`, `claude` (Claude Code) |
| Host imports (user) | `git-config`, `ssh-keys`, `claude-config`, `fish-config`, `zsh-config`, `neovim-config` |
| Shells (shell) | `bash`, `fish`, `zsh` |

Install them via the wizard, or explicitly:

```bash
isola create my-app --plugins rust,nodejs,git-config
```

## Where plugins come from

Plugins are discovered from three sources. When two define the same name, the
later one **wins**, so you can override a bundled plugin:

1. **bundled** — ship with isola.
2. **user** — `~/.isola/plugins/<name>/` (available to all your sandboxes).
3. **project** — `<project>/.isola/plugins/<name>/` (committed with a repo).

## Writing your own plugin

A plugin is just a directory with a `plugin.yaml` manifest and (usually) an
`install.sh`. Here's a complete one — drop it in `~/.isola/plugins/cowsay/`:

```
~/.isola/plugins/cowsay/
├── plugin.yaml
└── install.sh
```

```yaml
# plugin.yaml
name: cowsay
description: "cowsay — a talking ASCII cow"
version: "1.0.0"
layer: project
provision:
  script: install.sh           # run as root during provisioning
  verify: "cowsay it-works"     # sanity check; runs with paths.bin on PATH
paths:
  bin: [/usr/games]            # added to PATH for enter, exec, and verify
```

```bash
# install.sh
set -euo pipefail
apt-get install -y --no-install-recommends cowsay
```

Then:

```bash
isola create demo --plugins cowsay
isola exec demo -- cowsay "the plugin works"
```

## Manifest reference

```yaml
name: string                     # unique plugin id
description: string              # shown in the wizard and `isola plugins`
version: string
layer: project | user | shell   # default: project

provision:
  script: install.sh            # optional; omit for config-only plugins
  verify: "<command>"           # optional; run after install to sanity-check

paths:
  bin:   [/usr/games]                                  # dirs added to the sandbox PATH
  copy:  [{from: /root/.cargo, to: /home/sandbox/.cargo}]   # copy inside the rootfs (provision)
  host_copy: [{from: .config/nvim, to: .config/nvim}]       # host $HOME → sandbox (provision)
  host_mount: [{from: .ssh, to: .ssh, readonly: true}]      # host $HOME → sandbox (live, at entry)
  device: [{path: /dev/dri}]                            # /dev nodes to pass through (at entry)
  cache:  [{name: cargo, to: /home/sandbox/.cargo/registry}]  # shared package cache (at entry)

auto_detect:                    # user-layer only
  host_path: .gitconfig         # pre-select in the wizard if ~/<host_path> exists

prompts:                        # configurable values asked in the wizard
  - message: "PHP version:"
    env_var: PHP_VERSION        # exported before install.sh runs
    choices: ["8.4", "8.3", "8.2", "8.1"]   # omit for a free-text prompt
    default: "8.4"

shell:                          # shell-layer only
  bin: /usr/bin/fish            # the login shell binary inside the sandbox
  detect: fish                  # host $SHELL basename that pre-selects this shell
```

### When each field takes effect

- **provisioning time** (once, as in-namespace root): `prompts` are exported as
  env vars → `script` runs → `copy`/`host_copy` populate the home → `paths.bin`
  is added to PATH → `verify` runs.
- **entry time** (every `enter`/`exec`): `host_mount`, `device`, and `cache`
  are bind-mounted live; `paths.bin` is prepended to the session PATH.

`paths.bin` works everywhere — interactive `enter`, non-interactive `exec`, and
the provisioning `verify` step — so a plugin binary in a non-standard dir is
runnable by name in all of them.

## See also

- [Caching](caching.md) — how `paths.cache` shares package-manager downloads.
- [Security model](security.md) — what `host_mount` / `device` expose.
