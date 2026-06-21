# Caching

isola caches aggressively so that creating sandboxes and running builds are fast.
There are three independent caches, all under `~/.isola/cache/`.

## 1. Base rootfs (download cache)

The Ubuntu 24.04 base tarball is downloaded **once**, checksum-verified against
Ubuntu's published `SHA256SUMS`, and reused for every sandbox you ever create.

- Location: `~/.isola/cache/ubuntu-base-*.tar.gz`

## 2. Provisioned rootfs (layer cache)

Provisioning (installing your tools) is the slow part. isola captures the result
so that re-creating the **same** configuration is near-instant instead of
re-running `apt`/`cargo`/`npm`.

It works in layers:

- a **base layer** (system packages + the sandbox user + your shell), keyed by
  shell;
- one **env layer per plugin** (e.g. `rust`, `nodejs`), keyed by a hash of that
  plugin's install script.

When you create a sandbox, isola extracts whatever layers already match and only
builds the ones that are missing — so adding one new tool to a config you've used
before only rebuilds that one layer.

- Location: `~/.isola/cache/layers/`

## 3. Shared package caches

Even inside builds, isola avoids re-downloading packages by bind-mounting a
shared, per-package-manager cache into every sandbox at the tool's default cache
path. Downloads are reused **across all your sandboxes and sessions**.

| Plugin | Shared pool (host) | Mounted at | When |
|---|---|---|---|
| `rust` | `~/.isola/cache/pkg/cargo` | `~/.cargo/registry` | enter / exec |
| `nodejs` | `~/.isola/cache/pkg/npm` | `~/.npm` | enter / exec |
| `go` | `~/.isola/cache/pkg/{go-mod,go-build}` | `~/go/pkg/mod`, `~/.cache/go-build` | enter / exec |
| `python-uv` | `~/.isola/cache/pkg/uv` | `~/.cache/uv` | enter / exec |
| `apt` (base) | `~/.isola/cache/pkg/apt` | `/var/cache/apt/archives` | provisioning |

The toolchain caches speed up *your builds*; the apt cache means provisioning a
new configuration reuses already-downloaded `.deb`s.

These are **declared per plugin** via a `cache:` block in `plugin.yaml`, so your
own plugins can opt in:

```yaml
paths:
  cache:
    - name: cargo                          # pool at ~/.isola/cache/pkg/cargo
      to: /home/sandbox/.cargo/registry    # mount point inside the sandbox
```

(The `apt` cache is built in, because apt is part of the base system rather than
a plugin.) See [plugins.md](plugins.md) for the full manifest reference.

## How they fit together

- The **download cache** makes the first-ever sandbox fast.
- The **layer cache** makes re-creating the *same* sandbox fast.
- The **package caches** make *builds and new configurations* fast.

## Clearing caches

```bash
isola cache clean          # clear the shared package caches (~/.isola/cache/pkg)
isola cache clean --all    # also clear the rootfs / layer caches
```

The apt cache is written by provisioning as in-namespace root, so its files are
owned by a subordinate UID your host user can't delete directly. `isola cache
clean` removes them for you through a user namespace — don't `rm -rf` that
directory by hand.
