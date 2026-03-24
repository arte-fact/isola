mod cli;
mod commands;
mod error;
mod paths;
mod sandbox;

use clap::{CommandFactory, Parser};
use cli::{Cli, Commands};

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Some(Commands::Create { name, workspace }) => commands::create::run(&name, workspace),
        Some(Commands::Enter {
            name,
            shell,
            workspace,
        }) => match commands::enter::run(&name, shell, workspace) {
            Ok(code) => std::process::exit(code),
            Err(e) => Err(e),
        },
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
            match commands::enter::run(&name, true, None) {
                Ok(code) => std::process::exit(code),
                Err(e) => Err(e),
            }
        }
        Some(Commands::Exec {
            name,
            workspace,
            command,
        }) => match commands::exec::run(&name, command, workspace) {
            Ok(code) => std::process::exit(code),
            Err(e) => Err(e),
        },
        Some(Commands::Status { name }) => commands::status::run(&name),
        Some(Commands::Reprovision { name }) => commands::reprovision::run(&name),
        Some(Commands::Destroy { name }) => commands::destroy::run(&name),
        Some(Commands::List) => commands::list::run(),
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
