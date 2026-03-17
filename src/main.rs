mod adapters;
mod cache;
mod cli;
mod git;
mod info;
mod local_config;
mod lockfile;
mod manifest;
mod outdated;
mod relay;
mod report;
mod resolver;
mod review;
mod selection;
mod store;
mod update;

fn main() -> std::process::ExitCode {
    cli::run()
}
