use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(author, version, about = "Agen manages project-scoped agent packages", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Init,
    Sync {
        #[arg(long)]
        locked: bool,
        #[arg(long = "allow-high-sensitivity")]
        allow_high_sensitivity: bool,
    },
    Doctor,
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Init => crate::manifest::scaffold_init(),
        Command::Sync {
            locked,
            allow_high_sensitivity,
        } => crate::resolver::sync(locked, allow_high_sensitivity),
        Command::Doctor => crate::resolver::doctor(),
    }
}
