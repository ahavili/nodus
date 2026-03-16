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
    Add {
        url: String,
        #[arg(long)]
        tag: Option<String>,
    },
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
        Command::Add { url, tag } => crate::git::add_dependency(&url, tag.as_deref()),
        Command::Init => crate::manifest::scaffold_init(),
        Command::Sync {
            locked,
            allow_high_sensitivity,
        } => crate::resolver::sync(locked, allow_high_sensitivity),
        Command::Doctor => crate::resolver::doctor(),
    }
}
