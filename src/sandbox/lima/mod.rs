pub mod template;

use std::path::Path;
use std::process::Command;

use crate::error::IsolaError;
use crate::paths;
use crate::sandbox::backend::SandboxBackend;
use crate::sandbox::rootfs;

pub struct LimaBackend;

impl LimaBackend {
    /// Lima VM name for a given sandbox (prefixed to avoid collisions).
    fn vm_name(sandbox_name: &str) -> String {
        format!("isola-{sandbox_name}")
    }

    /// Check if limactl is installed and return its path.
    fn check_limactl() -> Result<(), IsolaError> {
        let status = Command::new("limactl")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        match status {
            Ok(s) if s.success() => Ok(()),
            _ => Err(IsolaError::ConfigError(
                "Lima is not installed. Install with: brew install lima".to_string(),
            )),
        }
    }

    /// Check if a VM exists.
    fn vm_exists(vm_name: &str) -> bool {
        Command::new("limactl")
            .args(["list", "--json"])
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    let text = String::from_utf8_lossy(&o.stdout);
                    // limactl list --json outputs one JSON object per line
                    Some(
                        text.lines()
                            .any(|line| line.contains(&format!("\"name\":\"{vm_name}\""))),
                    )
                } else {
                    None
                }
            })
            .unwrap_or(false)
    }

    /// Check if a VM is running.
    fn vm_running(vm_name: &str) -> bool {
        Command::new("limactl")
            .args(["list", "--json"])
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    let text = String::from_utf8_lossy(&o.stdout);
                    Some(text.lines().any(|line| {
                        line.contains(&format!("\"name\":\"{vm_name}\""))
                            && line.contains("\"status\":\"Running\"")
                    }))
                } else {
                    None
                }
            })
            .unwrap_or(false)
    }

    /// Ensure the VM is running, starting it if needed.
    fn ensure_vm_running(sandbox_name: &str) -> Result<(), IsolaError> {
        let vm = Self::vm_name(sandbox_name);
        if Self::vm_running(&vm) {
            return Ok(());
        }

        if !Self::vm_exists(&vm) {
            return Err(IsolaError::SandboxNotFound(sandbox_name.to_string()));
        }

        eprintln!("Starting VM '{vm}'...");
        let status = Command::new("limactl")
            .args(["start", &vm])
            .status()
            .map_err(|e| IsolaError::ConfigError(format!("Failed to start Lima VM: {e}")))?;

        if !status.success() {
            return Err(IsolaError::ConfigError(format!(
                "Failed to start Lima VM '{vm}'"
            )));
        }

        Ok(())
    }

    /// Build environment variable exports for passing into the VM.
    fn build_env_exports() -> String {
        let mut exports = Vec::new();

        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            exports.push(format!("export ANTHROPIC_API_KEY='{key}'"));
        }

        for var in &[
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
            "AWS_REGION",
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SESSION_TOKEN",
        ] {
            if let Ok(val) = std::env::var(var) {
                exports.push(format!("export {var}='{val}'"));
            }
        }

        if let Ok(term) = std::env::var("TERM") {
            exports.push(format!("export TERM='{term}'"));
        }

        for var in &["COLORTERM", "FORCE_COLOR", "NO_COLOR", "CLICOLOR_FORCE"] {
            if let Ok(val) = std::env::var(var) {
                exports.push(format!("export {var}='{val}'"));
            }
        }

        if exports.is_empty() {
            String::new()
        } else {
            exports.join(" && ") + " && "
        }
    }

    /// Build the bash script that writes config files inside the VM.
    fn build_setup_files_script(
        name: &str,
        environments: &[String],
        isolation_desc: &str,
    ) -> String {
        let claude_settings = serde_json::json!({
            "permissions": {
                "defaultMode": "bypassPermissions",
                "allow": [
                    "Bash", "Read", "Edit", "Write", "Glob", "Grep",
                    "WebFetch", "WebSearch", "Agent", "NotebookEdit"
                ],
                "deny": []
            }
        });
        let settings_json = serde_json::to_string_pretty(&claude_settings).unwrap();
        let claude_md = rootfs::build_claude_md(environments, isolation_desc);

        // Read host git config
        let git_name = Command::new("git")
            .args(["config", "--global", "user.name"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());

        let git_email = Command::new("git")
            .args(["config", "--global", "user.email"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());

        let mut script = format!(
            r#"#!/bin/bash
set -e

# Setup hostname
echo "{name}" > /etc/hostname
echo "127.0.0.1 localhost {name}" > /etc/hosts
echo "::1 localhost {name}" >> /etc/hosts

# APT sandbox config
mkdir -p /etc/apt/apt.conf.d
echo 'APT::Sandbox::User "root";' > /etc/apt/apt.conf.d/99sandbox

# Create directories
mkdir -p /home/sandbox/.claude /root/.claude /workspace /usr/local/bin

# Claude settings
cat > /home/sandbox/.claude/settings.json << 'ISOLA_SETTINGS_EOF'
{settings_json}
ISOLA_SETTINGS_EOF
cp /home/sandbox/.claude/settings.json /root/.claude/settings.json

# CLAUDE.md
cat > /home/sandbox/.claude/CLAUDE.md << 'ISOLA_MD_EOF'
{claude_md}
ISOLA_MD_EOF
cp /home/sandbox/.claude/CLAUDE.md /workspace/CLAUDE.md

# Session credentials symlink
mkdir -p /tmp/isola-session
ln -sf /tmp/isola-session/.credentials.json /home/sandbox/.claude/.credentials.json
"#
        );

        // Git config
        if git_name.is_some() || git_email.is_some() {
            script.push_str("cat > /home/sandbox/.gitconfig << 'ISOLA_GIT_EOF'\n[user]\n");
            if let Some(ref n) = git_name {
                script.push_str(&format!("\tname = {n}\n"));
            }
            if let Some(ref e) = git_email {
                script.push_str(&format!("\temail = {e}\n"));
            }
            script.push_str("ISOLA_GIT_EOF\ncp /home/sandbox/.gitconfig /root/.gitconfig\n");
        }

        script
            .push_str("\n# Fix ownership\nchown -R 1000:1000 /home/sandbox/ 2>/dev/null || true\n");

        script
    }
}

impl SandboxBackend for LimaBackend {
    fn preflight_checks(&self) -> Result<(), IsolaError> {
        Self::check_limactl()
    }

    fn create_environment(&self, name: &str, workspace: Option<&Path>) -> Result<(), IsolaError> {
        let vm = Self::vm_name(name);
        let sandbox_dir = paths::sandbox_dir(name);
        std::fs::create_dir_all(&sandbox_dir)?;

        // Ensure session directory exists
        let session_dir = paths::session_dir();
        std::fs::create_dir_all(&session_dir)?;
        let session_creds = paths::session_credentials();
        if !session_creds.exists() {
            std::fs::File::create(&session_creds)?;
        }

        // Generate Lima YAML
        let yaml = template::generate_lima_yaml(workspace, &session_dir);
        let yaml_path = sandbox_dir.join("lima.yaml");
        std::fs::write(&yaml_path, &yaml)?;

        eprintln!("Creating Lima VM '{vm}'...");
        let status = Command::new("limactl")
            .args([
                "create",
                "--name",
                &vm,
                "--tty=false",
                &yaml_path.to_string_lossy(),
            ])
            .status()
            .map_err(|e| IsolaError::ConfigError(format!("Failed to create Lima VM: {e}")))?;

        if !status.success() {
            return Err(IsolaError::ConfigError(format!(
                "Failed to create Lima VM '{vm}'"
            )));
        }

        eprintln!("Starting Lima VM '{vm}'...");
        let status = Command::new("limactl")
            .args(["start", &vm])
            .status()
            .map_err(|e| IsolaError::ConfigError(format!("Failed to start Lima VM: {e}")))?;

        if !status.success() {
            return Err(IsolaError::ConfigError(format!(
                "Failed to start Lima VM '{vm}'"
            )));
        }

        Ok(())
    }

    fn write_sandbox_files(&self, name: &str, environments: &[String]) -> Result<(), IsolaError> {
        let script =
            Self::build_setup_files_script(name, environments, self.isolation_description());
        let exit_code = self.run_command(name, &script)?;
        if exit_code != 0 {
            return Err(IsolaError::ConfigError(
                "Failed to write sandbox configuration files".to_string(),
            ));
        }
        Ok(())
    }

    fn enter_interactive(
        &self,
        name: &str,
        shell: bool,
        _workspace: Option<&Path>,
        _devices: Vec<String>,
    ) -> Result<i32, IsolaError> {
        Self::ensure_vm_running(name)?;
        let vm = Self::vm_name(name);
        let env_exports = Self::build_env_exports();
        let config = crate::sandbox::config::SandboxConfig::load(name)?;

        let sandbox_path = "/home/sandbox/.cargo/bin:/home/sandbox/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

        let status = if shell {
            Command::new("limactl")
                .args(["shell", &vm, "--", "sudo", "-i"])
                .status()
        } else {
            let shell_bin = config.shell.bin_path();
            let cmd = format!(
                "export PATH='{sandbox_path}' && {env_exports}cd /workspace 2>/dev/null; exec {shell_bin} -l"
            );
            Command::new("limactl")
                .args([
                    "shell", &vm, "--", "sudo", "-u", "sandbox", "-i", "bash", "-c", &cmd,
                ])
                .status()
        };

        match status {
            Ok(s) => Ok(s.code().unwrap_or(1)),
            Err(e) => Err(IsolaError::ConfigError(format!(
                "Failed to enter Lima VM: {e}"
            ))),
        }
    }

    fn run_command(&self, name: &str, command: &str) -> Result<i32, IsolaError> {
        Self::ensure_vm_running(name)?;
        let vm = Self::vm_name(name);

        let status = Command::new("limactl")
            .args(["shell", &vm, "--", "sudo", "bash", "-c", command])
            .status()
            .map_err(|e| {
                IsolaError::ConfigError(format!("Failed to run command in Lima VM: {e}"))
            })?;

        Ok(status.code().unwrap_or(1))
    }

    fn exec_command(
        &self,
        name: &str,
        command: &[String],
        _workspace: Option<&Path>,
        _devices: Vec<String>,
    ) -> Result<i32, IsolaError> {
        if command.is_empty() {
            return Err(IsolaError::ConfigError("no command specified".to_string()));
        }

        Self::ensure_vm_running(name)?;
        let vm = Self::vm_name(name);
        let env_exports = Self::build_env_exports();

        let cmd_str = command
            .iter()
            .map(|s| shell_escape(s))
            .collect::<Vec<_>>()
            .join(" ");
        let full_cmd = format!("{env_exports}cd /workspace 2>/dev/null; exec {cmd_str}");

        let status = Command::new("limactl")
            .args([
                "shell", &vm, "--", "sudo", "-u", "sandbox", "-i", "bash", "-c", &full_cmd,
            ])
            .status()
            .map_err(|e| {
                IsolaError::ConfigError(format!("Failed to exec command in Lima VM: {e}"))
            })?;

        Ok(status.code().unwrap_or(1))
    }

    fn destroy(&self, name: &str) -> Result<(), IsolaError> {
        let vm = Self::vm_name(name);

        // Stop VM (ignore errors if already stopped or doesn't exist)
        let _ = Command::new("limactl")
            .args(["stop", &vm])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        // Delete VM
        let _ = Command::new("limactl")
            .args(["delete", &vm, "--force"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        // Remove sandbox directory
        let sandbox_dir = paths::sandbox_dir(name);
        if sandbox_dir.exists() {
            std::fs::remove_dir_all(&sandbox_dir)?;
        }

        eprintln!("Sandbox '{}' destroyed", name);
        Ok(())
    }

    fn is_healthy(&self, name: &str) -> bool {
        let vm = Self::vm_name(name);
        Self::vm_exists(&vm)
    }

    fn backend_name(&self) -> &'static str {
        "lima-vm"
    }

    fn rootfs_url(&self) -> &'static str {
        template::CLOUD_IMAGE_URL
    }

    fn build_provision_script(&self, environments: &[String]) -> String {
        use crate::plugin::PluginRegistry;
        use crate::sandbox::config::SandboxShell;
        let registry = PluginRegistry::load().expect("failed to load plugin registry");
        let mut script =
            rootfs::build_provision_script(environments, &SandboxShell::default(), &registry);

        // Always install Node.js (needed for Claude CLI) if not already selected
        if !environments.iter().any(|e| e == "nodejs") {
            script.push_str(
                r#"
echo ">>> Installing Node.js (for Claude CLI)..."
curl -fsSL https://deb.nodesource.com/setup_22.x | bash -
apt-get install -y nodejs || true
dpkg --configure -a --force-overwrite 2>/dev/null || true
"#,
            );
        }

        // Install Claude CLI
        script.push_str(
            r#"
echo ">>> Installing Claude CLI..."
npm install -g @anthropic-ai/claude-code || true
"#,
        );

        script
    }

    fn isolation_description(&self) -> &'static str {
        "an isolated Linux VM (Lima + Apple Virtualization.framework)"
    }
}

/// Simple shell escaping for command arguments.
fn shell_escape(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '/' || c == '.' || c == ':')
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}
