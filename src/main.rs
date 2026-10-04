//! workforest: one git worktree per repo involved in a piece of work, so the
//! work happens in isolation from the repos' main checkouts.

mod cache;
mod cli;
mod commands;
mod config;
mod error;
mod fire;
mod forest;
mod git;
mod repos;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    match commands::run(cli::Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("workforest: {err}");
            ExitCode::FAILURE
        }
    }
}
