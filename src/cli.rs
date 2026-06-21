use clap::{Parser, Subcommand};
use clap_complete::Shell;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "isola",
    about = "Persistent isolated Linux sandboxes for developers"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Create a new persistent sandbox
    Create {
        /// Name for the sandbox
        name: String,
        /// Workspace directory to bind-mount into the sandbox
        #[arg(short, long)]
        workspace: Option<PathBuf>,
        /// Skip provisioning cache (force fresh provisioning)
        #[arg(long)]
        no_cache: bool,
        /// Comma-separated plugins to install (default: all project plugins)
        #[arg(long, value_delimiter = ',')]
        plugins: Vec<String>,
    },
    /// Enter a sandbox shell by name (from anywhere).
    ///
    /// In a project directory, just run `isola` with no arguments to enter the
    /// sandbox for the current directory.
    Enter {
        /// Name of the sandbox to enter
        name: String,
        /// Workspace directory to bind-mount (overrides config)
        #[arg(short, long)]
        workspace: Option<PathBuf>,
        /// Device nodes to bind-mount from host (e.g., /dev/kfd, /dev/dri)
        #[arg(long)]
        device: Vec<String>,
    },
    /// Run a command inside a sandbox
    Exec {
        /// Name of the sandbox
        name: String,
        /// Workspace directory to bind-mount (overrides config)
        #[arg(short, long)]
        workspace: Option<PathBuf>,
        /// Device nodes to bind-mount from host (e.g., /dev/kfd, /dev/dri)
        #[arg(long)]
        device: Vec<String>,
        /// Command and arguments to run
        #[arg(trailing_var_arg = true, required = true)]
        command: Vec<String>,
    },
    /// Show status and health of a sandbox
    Status {
        /// Name of the sandbox
        name: String,
    },
    /// Re-run provisioning on an existing sandbox
    Reprovision {
        /// Name of the sandbox
        name: String,
    },
    /// Destroy a sandbox and delete its rootfs
    Destroy {
        /// Name of the sandbox to destroy
        name: String,
    },
    /// List all sandboxes
    List,
    /// Install AppArmor profile for user namespace support (Ubuntu)
    SetupHost,
    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        shell: Shell,
    },
    /// Manage isola's caches
    Cache {
        #[command(subcommand)]
        action: CacheAction,
    },
}

#[derive(Subcommand)]
pub enum CacheAction {
    /// Remove cached downloads (shared package caches; --all also clears rootfs caches)
    Clean {
        /// Also remove the provisioned-rootfs / layer caches
        #[arg(long)]
        all: bool,
    },
}
