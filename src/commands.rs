//! What each subcommand does.

use std::fs;
use std::path::{Path, PathBuf};

use clap::CommandFactory;

use crate::cli::{
    Branching, BurnArgs, Cli, Command, CutArgs, ForestArg, LsArgs, NewArgs, PlantArgs, Removal,
};
use crate::config::Config;
use crate::error::{Context, Result, bail};
use crate::forest::{Forest, Tree, cwd_is_within};
use crate::git;

/// How to name a forest to commands that take it with `-f`.
const NAME_WITH_FLAG: &str = "pass -f <forest>";
/// How to name a forest to commands that take it as an argument.
const NAME_AS_ARGUMENT: &str = "pass the forest's name";

/// Run a parsed command line.
pub fn run(cli: Cli) -> Result<()> {
    let Some(command) = cli.command else {
        Cli::command()
            .print_help()
            .context("could not print help")?;
        return Ok(());
    };
    match command {
        Command::New(args) => new(&Config::from_env()?, args)?,
        Command::Plant(args) => plant(&Config::from_env()?, args)?,
        Command::Cut(args) => cut(&Config::from_env()?, args)?,
        Command::Burn(args) => burn(&Config::from_env()?, args)?,
        Command::Ls(args) => ls(&Config::from_env()?, args)?,
        Command::Status(args) => status(&Config::from_env()?, args)?,
        Command::Path(args) => path(&Config::from_env()?, args)?,
    }
    Ok(())
}

fn new(config: &Config, args: NewArgs) -> Result<()> {
    let sources = main_worktrees(&args.repos)?;
    let forest = Forest::create(config, &args.forest)?;
    println!("new forest {} at {}", forest.name, forest.dir.display());
    if sources.is_empty() {
        return Ok(());
    }
    plant_sources(&forest, sources, args.branching)
}

fn plant(config: &Config, args: PlantArgs) -> Result<()> {
    let forest = Forest::resolve(config, args.target.forest.as_deref(), NAME_WITH_FLAG)?;
    let sources = main_worktrees(&args.repos)?;
    plant_sources(&forest, sources, args.branching)
}

/// The main worktree of each repo argument, all checked before anything changes.
fn main_worktrees(repos: &[String]) -> Result<Vec<PathBuf>> {
    repos.iter().map(|arg| git::main_worktree(arg)).collect()
}

/// Add a worktree of each repo to `forest`, all on the same branch.
fn plant_sources(forest: &Forest, sources: Vec<PathBuf>, branching: Branching) -> Result<()> {
    let branch = branching.branch.unwrap_or_else(|| forest.name.clone());
    for source in sources {
        let repo = dir_name(&source);
        let dir = forest.tree_dir(&repo);
        if dir.symlink_metadata().is_ok() {
            bail!("already planted: {}", dir.display());
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
            "planted {repo} -> {} (branch {branch}, off {base})",
            dir.display()
        );
    }
    Ok(())
}

fn cut(config: &Config, args: CutArgs) -> Result<()> {
    let forest = Forest::resolve(config, args.target.forest.as_deref(), NAME_WITH_FLAG)?;
    for arg in &args.trees {
        let repo = dir_name(Path::new(arg));
        let Some(tree) = forest.tree(&repo)? else {
            bail!("{repo} is not a tree in {}", forest.name);
        };
        if !args.removal.force
            && let Some(risk) = git::unlanded_work(&forest.tree_dir(&tree.repo), &tree.base)
        {
            bail!("{repo} has {risk}; commit/push it or re-run with --force");
        }
        let dir = forest.tree_dir(&tree.repo);
        let standing_in_it = cwd_is_within(&dir);
        remove_tree(&forest, &tree, &args.removal)?;
        println!("cut {repo} from {}", forest.name);
        if standing_in_it {
            point_out_of(&dir, &tree.source);
        }
    }
    Ok(())
}

fn burn(config: &Config, args: BurnArgs) -> Result<()> {
    let forest = Forest::resolve(config, args.forest.as_deref(), NAME_AS_ARGUMENT)?;
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
    let way_out = forest.contains_cwd().then(|| {
        trees
            .iter()
            .find(|tree| cwd_is_within(&forest.tree_dir(&tree.repo)))
            .map_or_else(|| config.forest_root.clone(), |tree| tree.source.clone())
    });
    for tree in &trees {
        remove_tree(&forest, tree, &args.removal)?;
    }
    fs::remove_dir_all(&forest.dir)
        .context(format!("could not remove {}", forest.dir.display()))?;
    println!("burned forest {}", forest.name);
    if let Some(way_out) = way_out {
        point_out_of(&forest.dir, &way_out);
    }
    Ok(())
}

/// Tell the user how to leave a directory that was just deleted from under them.
fn point_out_of(gone: &Path, way_out: &Path) {
    eprintln!(
        "workforest: your shell is still in {}, which no longer exists; cd {}",
        gone.display(),
        way_out.display()
    );
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
