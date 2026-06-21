//! CLAUDE.md generation for the macOS/Lima backend (the Linux backend doesn't
//! write a CLAUDE.md into the rootfs).

const CLAUDE_MD_RUST: &str = r#"
## Rust
- Build: `cargo build`, Test: `cargo test`, Lint: `cargo clippy -- -D warnings`, Format: `cargo fmt`
- Use `thiserror` for library error types and `anyhow` for application-level errors; propagate with `?`
- Never use `.unwrap()` in library code; use `.expect("reason")` only for true invariants
- Prefer borrowing (`&T`, `&mut T`) over taking ownership; use `Cow<'_, str>` for conditional ownership
- Use `Vec::with_capacity()` when the size is known; prefer `&str` over `String` where possible
- Organize imports: std → external crates → local modules; no wildcard imports except preludes
- Derive common traits (`Debug`, `Clone`, `PartialEq`) on public types
- Never commit `dbg!()` or `println!()` debug statements
- Run `cargo fmt` and `cargo clippy` before committing
"#;

const CLAUDE_MD_NODEJS: &str = r#"
## Node.js
- Use ES modules (`import`/`export`), not CommonJS (`require`)
- Destructure imports when possible: `import { foo } from 'bar'`
- Run `npm test` to run the test suite; prefer running single test files over the full suite for speed
- Use `npm run lint` or `npx eslint .` for linting; use `npx prettier --write .` for formatting
- Enable TypeScript strict mode (`"strict": true` in tsconfig.json) when using TypeScript
- Use `async`/`await` over raw Promises or callbacks
- Pin dependency versions in `package.json`; run `npm ci` for reproducible installs
- Never commit `node_modules/` or `.env` files
"#;

const CLAUDE_MD_PYTHON_UV: &str = r#"
## Python
- Use `uv` exclusively for package management — never use pip, pip-tools, poetry, or conda
- Install: `uv add <package>`, Remove: `uv remove <package>`, Sync: `uv sync`, Lock: `uv lock`
- Run scripts with `uv run <script>.py`; run tools with `uv run <tool>` (pytest, ruff, mypy)
- Launch a REPL with `uv run python`
- Use `uv run ruff check .` for linting and `uv run ruff format .` for formatting
- Use type hints on all function signatures; validate with `uv run mypy .`
- Use `uv run pytest` to run tests; prefer `uv run pytest path/to/test.py` for single files
- Never use bare `python` or `pip` commands — always go through `uv run`
"#;

const CLAUDE_MD_GO: &str = r#"
## Go
- Build: `go build ./...`, Test: `go test ./...`, Lint: `golangci-lint run` (if installed)
- Format with `gofmt` — code must always be gofmt-compliant
- Follow "accept interfaces, return structs" for flexible API design
- Use explicit error handling with return values; check every error, never discard with `_`
- Use `context.Context` as the first parameter for functions that do I/O or may be cancelled
- Prefer table-driven tests with `t.Run()` subtests
- Use `go vet ./...` before committing to catch common mistakes
- Standard project layout: `cmd/` for entrypoints, `internal/` for private packages, `pkg/` for public libraries
"#;

/// Build CLAUDE.md content based on selected environments.
pub fn build_claude_md(environments: &[String], isolation_desc: &str) -> String {
    let mut md = format!(
        r#"# Sandbox Environment

You are running inside {isolation_desc}.

## Environment
- **OS**: Ubuntu 24.04 base"#
    );
    md.push_str(
        r#"
- **User**: sandbox (non-root)
- **Network**: Full unrestricted internet access (shared with host)
- **Workspace**: /workspace (bind-mounted from host project directory, read-write)

## Running privileged commands
Use sudo with the password "sandbox" for any privileged operation:
```
echo "sandbox" | sudo -S apt-get install -y <package>
echo "sandbox" | sudo -S <command>
```

## Available Tools
"#,
    );

    for env in environments {
        match env.as_str() {
            "rust" => md.push_str("- **Rust**: rustc + cargo (`/home/sandbox/.cargo/bin/`)\n"),
            "nodejs" => md.push_str("- **Node.js**: v22 LTS (`/usr/bin/node`, `/usr/bin/npm`)\n"),
            "python-uv" => md.push_str(
                "- **Python**: python3 + uv (`/usr/bin/python3`, `/home/sandbox/.local/bin/uv`)\n",
            ),
            "go" => md.push_str("- **Go**: (`/usr/local/go/bin/go`)\n"),
            _ => {}
        }
    }

    md.push_str("- **System**: use sudo to install any additional packages with apt-get\n");

    for env in environments {
        match env.as_str() {
            "rust" => md.push_str(CLAUDE_MD_RUST),
            "nodejs" => md.push_str(CLAUDE_MD_NODEJS),
            "python-uv" => md.push_str(CLAUDE_MD_PYTHON_UV),
            "go" => md.push_str(CLAUDE_MD_GO),
            _ => {}
        }
    }

    md.push_str(
        r#"
## Important
- Changes to `/workspace` are reflected on the host filesystem immediately.
- Changes outside `/workspace` persist across sandbox sessions (persistent sandbox).
- You cannot see or affect host processes. Your PID namespace is isolated.
- You are free to run any command without restriction.
"#,
    );

    md
}
