//! workforest: one git worktree per repo involved in a piece of work, so the
//! work happens in isolation from the repos' main checkouts.

mod cache;
mod cli;
mod commands;
mod complete;
mod config;
mod error;
mod fire;
mod forest;
mod git;
mod repos;

use std::process::ExitCode;

use clap::{CommandFactory, Parser};

fn main() -> ExitCode {
    // Answers a shell asking for completions, then exits; otherwise returns.
    clap_complete::CompleteEnv::with_factory(cli::Cli::command).complete();
    match commands::run(cli::Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("workforest: {err}");
            ExitCode::FAILURE
        }
    }
}
