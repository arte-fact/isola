mod cli;
mod commands;
mod error;
mod paths;
mod plugin;
#[cfg(target_os = "linux")]
mod progress;
mod sandbox;
#[cfg(target_os = "linux")]
mod sha256;

use clap::{CommandFactory, Parser};
use cli::{Cli, Commands};

/// Reset terminal to a sane state after the sandbox child exits.
/// Interactive shells may leave the terminal in raw mode,
/// alternate screen, or with modified attributes.
pub fn reset_terminal() {
    // \x1b[?1049l  — leave alternate screen buffer
    // \x1b[?25h    — show cursor
    // \x1b[0m      — reset all attributes (colors, bold, etc.)
    eprint!("\x1b[?1049l\x1b[?25h\x1b[0m");

    // Best-effort: restore cooked mode via stty
    let _ = std::process::Command::new("stty")
        .arg("sane")
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Some(Commands::Create {
            name,
            workspace,
            no_cache,
            plugins,
        }) => commands::create::run(&name, workspace, no_cache, plugins),
        Some(Commands::Enter {
            name,
            workspace,
            device,
        }) => match commands::enter::run(&name, workspace, device) {
            Ok(code) => {
                reset_terminal();
                std::process::exit(code);
            }
            Err(e) => Err(e),
        },
        Some(Commands::Exec {
            name,
            workspace,
            device,
            command,
        }) => match commands::exec::run(&name, command, workspace, device) {
            Ok(code) => {
                reset_terminal();
                std::process::exit(code);
            }
            Err(e) => Err(e),
        },
        Some(Commands::Status { name }) => commands::status::run(&name),
        Some(Commands::Reprovision { name }) => commands::reprovision::run(&name),
        Some(Commands::Destroy { name }) => commands::destroy::run(&name),
        Some(Commands::List) => commands::list::run(),
        Some(Commands::SetupHost) => commands::setup_host::run(),
        Some(Commands::Completions { shell }) => {
            clap_complete::generate(shell, &mut Cli::command(), "isola", &mut std::io::stdout());
            Ok(())
        }
        Some(Commands::Cache { action }) => match action {
            cli::CacheAction::Clean { all } => commands::cache::clean(all),
        },
        Some(Commands::Plugins) => commands::plugins::list(),
        None => commands::default::run(),
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        let mut source = std::error::Error::source(&e);
        while let Some(cause) = source {
            eprintln!("  caused by: {cause}");
            source = cause.source();
        }
        std::process::exit(1);
    }
}
