mod cli;
mod commands;
mod error;
mod paths;
mod progress;
mod sandbox;

use clap::{CommandFactory, Parser};
use cli::{Cli, Commands};

/// Reset terminal to a sane state after the sandbox child exits.
/// Claude Code or interactive shells may leave the terminal in raw mode,
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
        }) => commands::create::run(&name, workspace, no_cache),
        Some(Commands::Enter {
            name,
            shell,
            claude,
            workspace,
        }) => {
            let force_claude = if claude { Some(true) } else { None };
            match commands::enter::run(&name, shell, force_claude, workspace) {
                Ok(code) => {
                    reset_terminal();
                    std::process::exit(code);
                }
                Err(e) => Err(e),
            }
        }
        Some(Commands::Shell { name }) => {
            let name = match name.or_else(|| commands::default::detect_sandbox().ok().flatten()) {
                Some(n) => n,
                None => {
                    eprintln!(
                        "error: no sandbox found for current directory. Create one first with: isola"
                    );
                    std::process::exit(1);
                }
            };
            match commands::enter::run(&name, true, None, None) {
                Ok(code) => {
                    reset_terminal();
                    std::process::exit(code);
                }
                Err(e) => Err(e),
            }
        }
        Some(Commands::Exec {
            name,
            workspace,
            command,
        }) => match commands::exec::run(&name, command, workspace) {
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
        None => commands::default::run(),
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
