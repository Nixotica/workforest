//! What each subcommand does.

use std::fs;
use std::io::{self, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process;

use clap::CommandFactory;

use crate::cli::{
    Branching, BurnArgs, Cli, Command, ExecArgs, ForestArg, GraftArgs, LsArgs, NewArgs, PruneArgs,
    Removal,
};
use crate::config::Config;
use crate::error::{Context, Result, bail};
use crate::forest::{Forest, Tree};
use crate::git;

/// How to name a forest to commands that take it with `-f`.
const NAME_WITH_FLAG: &str = "pass -f <forest>";
/// How to name a forest to commands that take it as an argument.
const NAME_AS_ARGUMENT: &str = "pass the forest's name";

/// Run a parsed command line, returning the process exit code.
pub fn run(cli: Cli) -> Result<u8> {
    let Some(command) = cli.command else {
        Cli::command()
            .print_help()
            .context("could not print help")?;
        return Ok(0);
    };
    match command {
        Command::New(args) => new(&Config::from_env()?, args)?,
        Command::Graft(args) => graft(&Config::from_env()?, args)?,
        Command::Prune(args) => prune(&Config::from_env()?, args)?,
        Command::Burn(args) => burn(&Config::from_env()?, args)?,
        Command::Ls(args) => ls(&Config::from_env()?, args)?,
        Command::Status(args) => status(&Config::from_env()?, args)?,
        Command::Path(args) => path(&Config::from_env()?, args)?,
        Command::Exec(args) => return exec(&Config::from_env()?, args),
    }
    Ok(0)
}

fn new(config: &Config, args: NewArgs) -> Result<()> {
    let sources = main_worktrees(&args.repos)?;
    let forest = Forest::plant(config, &args.forest)?;
    println!("planted forest {} at {}", forest.name, forest.dir.display());
    if sources.is_empty() {
        return Ok(());
    }
    graft_sources(&forest, sources, args.branching)
}

fn graft(config: &Config, args: GraftArgs) -> Result<()> {
    let forest = Forest::resolve(config, args.target.forest.as_deref(), NAME_WITH_FLAG)?;
    let sources = main_worktrees(&args.repos)?;
    graft_sources(&forest, sources, args.branching)
}

/// The main worktree of each repo argument, all checked before anything changes.
fn main_worktrees(repos: &[String]) -> Result<Vec<PathBuf>> {
    repos.iter().map(|arg| git::main_worktree(arg)).collect()
}

/// Add a worktree of each repo to `forest`, all on the same branch.
fn graft_sources(forest: &Forest, sources: Vec<PathBuf>, branching: Branching) -> Result<()> {
    let branch = branching.branch.unwrap_or_else(|| forest.name.clone());
    for source in sources {
        let repo = dir_name(&source);
        let dir = forest.tree_dir(&repo);
        if dir.symlink_metadata().is_ok() {
            bail!("already grafted: {}", dir.display());
        }
        let base = branching
            .base
            .clone()
            .unwrap_or_else(|| git::default_base(&source));
        git::add_worktree(&source, &dir, &branch, &base)?;
        forest.record(Tree {
            repo: repo.clone(),
            source,
            branch: branch.clone(),
            base: base.clone(),
        })?;
        println!(
            "grafted {repo} -> {} (branch {branch}, off {base})",
            dir.display()
        );
    }
    println!("forest {}: {}", forest.name, forest.dir.display());
    Ok(())
}

fn prune(config: &Config, args: PruneArgs) -> Result<()> {
    let forest = Forest::resolve(config, args.target.forest.as_deref(), NAME_WITH_FLAG)?;
    for arg in &args.repos {
        let repo = dir_name(Path::new(arg));
        let Some(tree) = forest.tree(&repo)? else {
            bail!("{repo} is not grafted into {}", forest.name);
        };
        if !args.removal.force
            && let Some(risk) = git::unlanded_work(&forest.tree_dir(&tree.repo), &tree.base)
        {
            bail!("{repo} has {risk}; commit/push it or re-run with --force");
        }
        remove_tree(&forest, &tree, &args.removal)?;
        println!("pruned {repo} from {}", forest.name);
    }
    Ok(())
}

fn burn(config: &Config, args: BurnArgs) -> Result<()> {
    let forest = Forest::named(config, &args.forest)?;
    if forest.contains_cwd() {
        bail!("cd out of {} before burning it", forest.dir.display());
    }
    let trees = forest.trees()?;
    if !args.removal.force {
        let mut risks: Vec<String> = trees
            .iter()
            .filter_map(|tree| {
                git::unlanded_work(&forest.tree_dir(&tree.repo), &tree.base)
                    .map(|risk| format!("  {}: {risk}", tree.repo))
            })
            .collect();
        risks.extend(
            forest
                .unrecorded_checkouts(&trees)?
                .into_iter()
                .map(|name| format!("  {name}: a checkout the manifest doesn't record")),
        );
        if !risks.is_empty() {
            bail!(
                "refusing to burn {} — work would be lost:\n{}\nre-run with --force to delete anyway",
                forest.name,
                risks.join("\n")
            );
        }
    }
    for tree in &trees {
        remove_tree(&forest, tree, &args.removal)?;
    }
    fs::remove_dir_all(&forest.dir)
        .context(format!("could not remove {}", forest.dir.display()))?;
    println!("burned forest {}", forest.name);
    Ok(())
}

/// Remove one tree's worktree, and its branch if asked, then forget it.
fn remove_tree(forest: &Forest, tree: &Tree, removal: &Removal) -> Result<()> {
    let dir = forest.tree_dir(&tree.repo);
    if dir.is_dir() {
        git::remove_worktree(&tree.source, &dir, removal.force)?;
    } else {
        git::prune_worktrees(&tree.source)?;
    }
    if removal.delete_branches
        && !tree.branch.is_empty()
        && !git::delete_branch(&tree.source, &tree.branch)
    {
        eprintln!(
            "workforest: could not delete branch {} in {}",
            tree.branch, tree.repo
        );
    }
    forest.forget(&tree.repo)
}

fn ls(config: &Config, args: LsArgs) -> Result<()> {
    if let Some(name) = args.forest {
        let forest = Forest::named(config, &name)?;
        println!("{}  {}", forest.name, forest.dir.display());
        for tree in forest.trees()? {
            println!(
                "  {:<24} {:<24} (off {})",
                tree.repo, tree.branch, tree.base
            );
        }
        return Ok(());
    }
    let forests = Forest::all(config)?;
    if forests.is_empty() {
        println!("no forests yet ({})", config.forest_root.display());
    }
    for forest in forests {
        println!("{:<28} {} tree(s)", forest.name, forest.trees()?.len());
    }
    Ok(())
}

fn status(config: &Config, args: ForestArg) -> Result<()> {
    let forest = Forest::resolve(config, args.forest.as_deref(), NAME_AS_ARGUMENT)?;
    println!("{}  {}", forest.name, forest.dir.display());
    for tree in forest.trees()? {
        let dir = forest.tree_dir(&tree.repo);
        if !dir.is_dir() {
            println!("  {:<24} MISSING", tree.repo);
            continue;
        }
        let state = match git::is_dirty(&dir) {
            Some(true) => "dirty",
            Some(false) => "clean",
            None => "?",
        };
        let ahead = count_or_unknown(&dir, &format!("{}..HEAD", tree.base));
        let behind = count_or_unknown(&dir, &format!("HEAD..{}", tree.base));
        println!(
            "  {:<24} {:<24} {state:<6} +{ahead}/-{behind} vs {}",
            tree.repo, tree.branch, tree.base
        );
    }
    Ok(())
}

fn path(config: &Config, args: ForestArg) -> Result<()> {
    let forest = Forest::resolve(config, args.forest.as_deref(), NAME_AS_ARGUMENT)?;
    println!("{}", forest.dir.display());
    Ok(())
}

/// Run the command in each tree in turn. Returns the exit code of the last
/// tree whose command failed, or 0 when none did.
fn exec(config: &Config, args: ExecArgs) -> Result<u8> {
    let forest = Forest::resolve(config, args.target.forest.as_deref(), NAME_WITH_FLAG)?;
    let Some((program, program_args)) = args.command.split_first() else {
        bail!("no command given");
    };
    let mut code = 0;
    for tree in forest.trees()? {
        let dir = forest.tree_dir(&tree.repo);
        if !dir.is_dir() {
            continue;
        }
        println!("\n=== {} ===", tree.repo);
        io::stdout().flush().context("could not write to stdout")?;
        match process::Command::new(program)
            .args(program_args)
            .current_dir(&dir)
            .status()
        {
            Ok(status) if status.success() => {}
            Ok(status) => code = failure_code(status),
            Err(err) => {
                eprintln!(
                    "workforest: could not run {}: {err}",
                    program.to_string_lossy()
                );
                code = 127;
            }
        }
    }
    Ok(code)
}

/// A failed command's exit code as a shell reports it: its own code, or 128
/// plus the signal that killed it.
fn failure_code(status: process::ExitStatus) -> u8 {
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .and_then(|code| u8::try_from(code).ok())
        .filter(|&code| code != 0)
        .unwrap_or(1)
}

fn count_or_unknown(dir: &Path, range: &str) -> String {
    git::count(dir, range).map_or_else(|| "?".to_owned(), |count| count.to_string())
}

/// The last component of `path`, which names a repo and its tree.
fn dir_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}
