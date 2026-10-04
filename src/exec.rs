//! `exec`: run one command in every tree of a forest.
//!
//! The command runs as given, not through a shell: `sh -c '...'` is the way to
//! use shell syntax. In order, each tree's output streams under a header as it
//! runs. In parallel, each tree's output and errors are gathered and printed
//! under its header as it finishes, so trees' output never interleaves.

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Mutex;
use std::thread;

use crate::error::{Result, bail};
use crate::forest::{Forest, Tree};

/// How running the command in one tree went.
enum Outcome {
    Succeeded,
    Failed(ExitStatus),
    NotStarted(io::Error),
    Missing,
}

impl Outcome {
    /// Why the tree failed, unless it didn't.
    fn failure(&self) -> Option<String> {
        match self {
            Outcome::Succeeded => None,
            Outcome::Failed(status) => Some(match status.code() {
                Some(code) => format!("exit status {code}"),
                None => status.to_string(),
            }),
            Outcome::NotStarted(err) => Some(format!("could not start: {err}")),
            Outcome::Missing => Some("its directory is gone".to_owned()),
        }
    }
}

/// Run `command` in every tree of `forest`, in order, or `parallel` trees at a
/// time. Fails, listing them, if it failed in any tree.
pub fn exec(forest: &Forest, command: &[OsString], parallel: Option<usize>) -> Result<()> {
    let trees = forest.trees()?;
    let outcomes = match parallel {
        None => trees
            .iter()
            .map(|tree| {
                header(&tree.repo);
                let outcome = run_streaming(forest, tree, command);
                (tree.repo.clone(), outcome)
            })
            .collect(),
        Some(at_once) => run_parallel(forest, &trees, command, at_once),
    };
    let failures: Vec<String> = outcomes
        .iter()
        .filter_map(|(repo, outcome)| outcome.failure().map(|why| format!("  {repo}: {why}")))
        .collect();
    if !failures.is_empty() {
        bail!(
            "failed in {} of {} tree(s):\n{}",
            failures.len(),
            outcomes.len(),
            failures.join("\n")
        );
    }
    Ok(())
}

fn header(repo: &str) {
    println!("=== {repo} ===");
    let _ = io::stdout().flush();
}

/// The command, set up to run in `tree`, which it is told about through
/// `WORKFOREST_FOREST` and `WORKFOREST_TREE`.
fn command_in(forest: &Forest, tree: &Tree, command: &[OsString]) -> Command {
    let mut process = Command::new(&command[0]);
    process
        .args(&command[1..])
        .current_dir(forest.tree_dir(&tree.repo))
        .env("WORKFOREST_FOREST", &forest.name)
        .env("WORKFOREST_TREE", &tree.repo);
    process
}

/// Run in one tree with the terminal's stdin, stdout and stderr.
fn run_streaming(forest: &Forest, tree: &Tree, command: &[OsString]) -> Outcome {
    if !forest.tree_dir(&tree.repo).is_dir() {
        return Outcome::Missing;
    }
    let status = command_in(forest, tree, command).status();
    let outcome = finished(status);
    if let Outcome::NotStarted(err) = &outcome {
        eprintln!(
            "workforest: could not start {}: {err}",
            command[0].to_string_lossy()
        );
    }
    outcome
}

/// Run in up to `at_once` trees at a time, printing each tree's gathered
/// output under its header as it finishes, in the order they finish.
fn run_parallel(
    forest: &Forest,
    trees: &[Tree],
    command: &[OsString],
    at_once: usize,
) -> Vec<(String, Outcome)> {
    let queue = Mutex::new(trees.iter());
    let outcomes = Mutex::new(Vec::new());
    let printing = Mutex::new(());
    thread::scope(|scope| {
        for _ in 0..at_once.clamp(1, trees.len().max(1)) {
            scope.spawn(|| {
                loop {
                    let next = queue.lock().ok().and_then(|mut queue| queue.next());
                    let Some(tree) = next else {
                        break;
                    };
                    let (output, outcome) = run_gathered(forest, tree, command);
                    if let Ok(_turn) = printing.lock() {
                        header(&tree.repo);
                        let mut stdout = io::stdout().lock();
                        let _ = stdout.write_all(&output);
                        if let Outcome::NotStarted(err) = &outcome {
                            let _ = writeln!(
                                stdout,
                                "workforest: could not start {}: {err}",
                                command[0].to_string_lossy()
                            );
                        }
                        let _ = stdout.flush();
                    }
                    if let Ok(mut outcomes) = outcomes.lock() {
                        outcomes.push((tree.repo.clone(), outcome));
                    }
                }
            });
        }
    });
    outcomes.into_inner().unwrap_or_default()
}

/// Run in one tree with no stdin, gathering stdout and stderr together in the
/// order the command wrote them.
fn run_gathered(forest: &Forest, tree: &Tree, command: &[OsString]) -> (Vec<u8>, Outcome) {
    if !forest.tree_dir(&tree.repo).is_dir() {
        return (Vec::new(), Outcome::Missing);
    }
    let (mut reader, writer) = match io::pipe() {
        Ok(pipe) => pipe,
        Err(err) => return (Vec::new(), Outcome::NotStarted(err)),
    };
    let mut process = command_in(forest, tree, command);
    process.stdin(Stdio::null());
    let started = writer
        .try_clone()
        .and_then(|errors| process.stdout(writer).stderr(errors).spawn());
    // The command's copies of the pipe's write end close when it exits, and
    // this process's copies were moved into it, so reading ends with it.
    drop(process);
    let mut child = match started {
        Ok(child) => child,
        Err(err) => return (Vec::new(), Outcome::NotStarted(err)),
    };
    let mut output = Vec::new();
    let _ = reader.read_to_end(&mut output);
    (output, finished(child.wait()))
}

fn finished(status: io::Result<ExitStatus>) -> Outcome {
    match status {
        Ok(status) if status.success() => Outcome::Succeeded,
        Ok(status) => Outcome::Failed(status),
        Err(err) => Outcome::NotStarted(err),
    }
}

/// How many trees to run at once when `--parallel` gives no number: one per
/// CPU.
pub fn cpus() -> usize {
    thread::available_parallelism().map_or(1, usize::from)
}
