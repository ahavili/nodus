mod adapters;
mod cli;
mod lockfile;
mod manifest;
mod resolver;
mod state;
mod store;

fn main() -> anyhow::Result<()> {
    cli::run()
}
