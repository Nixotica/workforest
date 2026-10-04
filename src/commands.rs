//! What each subcommand does.

use std::fs;
use std::path::{Path, PathBuf};

use clap::CommandFactory;

use crate::cache;
use crate::cli::{
    Branching, BurnArgs, CacheCommand, CacheDoctorArgs, CacheDropArgs, CacheGraftArgs,
    CachePathsArgs, Caching, Cli, Command, CutArgs, FireArgs, ForestArg, LsArgs, NewArgs,
    PlantArgs, Removal,
};
use crate::config::{self, Config};
use crate::error::{Context, Result, bail};
use crate::fire;
use crate::forest::{Forest, Tree, cwd_is_within, dir_name};
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
        Command::Fire(args) => fire(&Config::from_env()?, args)?,
        Command::Ls(args) => ls(&Config::from_env()?, args)?,
        Command::Status(args) => status(&Config::from_env()?, args)?,
        Command::Path(args) => path(&Config::from_env()?, args)?,
        Command::Cache(args) => {
            let config = Config::from_env()?;
            match args.command {
                CacheCommand::Status(args) => cache_status(&config, args)?,
                CacheCommand::Graft(args) => cache_graft(&config, args)?,
                CacheCommand::Drop(args) => cache_drop(&config, args)?,
                CacheCommand::Paths(args) => cache_paths(args)?,
                CacheCommand::Doctor(args) => cache_doctor(&config, args)?,
            }
        }
    }
    Ok(())
}

fn new(config: &Config, args: NewArgs) -> Result<()> {
    let sources = main_worktrees(&args.repos)?;
    let graft = grafting(&args.caching)?;
    let forest = Forest::create(config, &args.forest)?;
    println!("new forest {} at {}", forest.name, forest.dir.display());
    if sources.is_empty() {
        return Ok(());
    }
    plant_sources(&forest, sources, args.branching, graft)
}

fn plant(config: &Config, args: PlantArgs) -> Result<()> {
    let forest = Forest::resolve(config, args.target.forest.as_deref(), NAME_WITH_FLAG)?;
    let sources = main_worktrees(&args.repos)?;
    let graft = grafting(&args.caching)?;
    plant_sources(&forest, sources, args.branching, graft)
}

/// The size from which grafted cache files are hardlinked, or `None` if
/// `--no-cache` says not to graft.
fn grafting(caching: &Caching) -> Result<Option<u64>> {
    if caching.no_cache {
        return Ok(None);
    }
    config::link_min().map(Some)
}

/// The main worktree of each repo argument, all checked before anything changes.
fn main_worktrees(repos: &[String]) -> Result<Vec<PathBuf>> {
    repos.iter().map(|arg| git::main_worktree(arg)).collect()
}

/// Add a worktree of each repo to `forest`, all on the same branch. Unless
/// `graft` is `None`, graft the build caches each repo declares, hardlinking
/// files of that many bytes and up.
fn plant_sources(
    forest: &Forest,
    sources: Vec<PathBuf>,
    branching: Branching,
    graft: Option<u64>,
) -> Result<()> {
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
            source: source.clone(),
            branch: branch.clone(),
            base: base.clone(),
        })?;
        println!(
            "planted {repo} -> {} (branch {branch}, off {base})",
            dir.display()
        );
        if let Some(link_min) = graft {
            cache::graft_tree(&dir, &source, link_min, false);
        }
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
        cut_tree(&forest, &tree, &args.removal)?;
    }
    Ok(())
}

/// Remove one tree from `forest`, saying where to go if the shell was in it.
fn cut_tree(forest: &Forest, tree: &Tree, removal: &Removal) -> Result<()> {
    let dir = forest.tree_dir(&tree.repo);
    let standing_in_it = cwd_is_within(&dir);
    remove_tree(forest, tree, removal)?;
    println!("cut {} from {}", tree.repo, forest.name);
    if standing_in_it {
        point_out_of(&dir, &tree.source);
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
    let removals: Vec<_> = trees.into_iter().map(|tree| (tree, args.removal)).collect();
    burn_forest(config, &forest, &removals)
}

/// Remove each tree of `forest` as its removal says, then the forest itself,
/// saying where to go if the shell was in it.
fn burn_forest(config: &Config, forest: &Forest, trees: &[(Tree, Removal)]) -> Result<()> {
    let way_out = forest.contains_cwd().then(|| {
        trees
            .iter()
            .map(|(tree, _)| tree)
            .find(|tree| cwd_is_within(&forest.tree_dir(&tree.repo)))
            .map_or_else(|| config.forest_root.clone(), |tree| tree.source.clone())
    });
    for (tree, removal) in trees {
        remove_tree(forest, tree, removal)?;
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

/// Remove one tree's worktree, and its branch if asked, then forget it. A tree
/// whose directory and repo are both gone only needs forgetting.
fn remove_tree(forest: &Forest, tree: &Tree, removal: &Removal) -> Result<()> {
    let dir = forest.tree_dir(&tree.repo);
    if dir.is_dir() {
        git::remove_worktree(&tree.source, &dir, removal.force)?;
    } else if tree.source.is_dir() {
        git::prune_worktrees(&tree.source)?;
    } else {
        return forest.forget(&tree.repo);
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

/// What `status` reports about a tree's working copy.
#[derive(Clone, Copy)]
enum TreeState {
    /// Git could not say.
    Unknown,
    /// Uncommitted changes.
    Dirty,
    /// No uncommitted changes, and work of its own that is on the base by now,
    /// however it was merged.
    Landed,
    /// No uncommitted changes.
    Clean,
}

impl TreeState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "?",
            Self::Dirty => "dirty",
            Self::Landed => "landed",
            Self::Clean => "clean",
        }
    }
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
        let ahead = git::count(&dir, &format!("{}..HEAD", tree.base));
        let behind = git::count(&dir, &format!("HEAD..{}", tree.base));
        let state = match git::is_dirty(&dir) {
            Some(true) => TreeState::Dirty,
            Some(false) if head_landed(&dir, &tree) => TreeState::Landed,
            Some(false) => TreeState::Clean,
            None => TreeState::Unknown,
        };
        // Landed work is safe to burn whether or not it was pushed.
        let push = match ahead {
            Some(ahead) if ahead > 0 && !matches!(state, TreeState::Landed) => {
                format!(", {}", push_state(&dir, ahead))
            }
            _ => String::new(),
        };
        println!(
            "  {:<24} {:<24} {:<6} +{}/-{} vs {}{push}",
            tree.repo,
            tree.branch,
            state.as_str(),
            count_or_unknown(ahead),
            count_or_unknown(behind),
            tree.base
        );
    }
    Ok(())
}

/// Whether `tree`, checked out at `dir`, holds work of its own that has landed
/// by now, as `fire` judges it.
fn head_landed(dir: &Path, tree: &Tree) -> bool {
    let branch = git::current_branch(dir);
    fire::landed_on(dir, tree, branch.as_deref()).is_some()
}

/// Whether the tree at `dir`, `ahead` commits ahead of its base, has them all
/// on its upstream, else how many its upstream lacks. As `burn` sees it, none
/// are pushed when the branch has no upstream.
fn push_state(dir: &Path, ahead: u64) -> String {
    let unpushed = if git::has_upstream(dir) {
        git::count(dir, "@{upstream}..HEAD")
    } else {
        Some(ahead)
    };
    match unpushed {
        Some(0) => "pushed".to_owned(),
        unpushed => format!("{} unpushed", count_or_unknown(unpushed)),
    }
}

fn fire(config: &Config, args: FireArgs) -> Result<()> {
    let forests = Forest::all(config)?;
    if !args.no_fetch {
        fire::fetch_bases(&forests);
    }
    let (mut burns, mut cuts, mut kept_with_dead, mut in_flight, mut failures) = (0, 0, 0, 0, 0);
    for forest in forests {
        let name = forest.name.clone();
        let judgement = match fire::judge_forest(forest) {
            Ok(judgement) => judgement,
            Err(err) => {
                eprintln!("workforest: {name}: {err}");
                failures += 1;
                continue;
            }
        };
        if judgement.is_in_flight() {
            in_flight += 1;
            continue;
        }
        let dead = judgement.is_dead();
        let has_dead = judgement.dead_trees().next().is_some();
        let cutting = !dead && has_dead && args.scorch;
        if !dead && has_dead && !args.scorch {
            kept_with_dead += 1;
        }
        println!(
            "{name}  {}",
            if dead {
                "burn"
            } else if cutting {
                "cut its dead trees"
            } else {
                "keep"
            }
        );
        let mut removals = Vec::new();
        for (tree, verdict) in &judgement.trees {
            let mut reason = verdict.reason().to_owned();
            if verdict.is_dead() && (dead || cutting) {
                let delete_branch = args.delete_branches && fire::branch_spent(tree);
                if args.delete_branches && !delete_branch {
                    reason.push_str(&format!(
                        "; keeps branch {}, which has commits not on {}",
                        tree.branch, tree.base
                    ));
                }
                let removal = Removal {
                    force: false,
                    delete_branches: delete_branch,
                };
                removals.push((tree.clone(), removal));
            }
            println!("  {:<24} {:<6} {reason}", tree.repo, verdict.label());
        }
        for stray in &judgement.strays {
            println!(
                "  {stray:<24} {:<6} a checkout the manifest doesn't record",
                "?"
            );
        }
        if dead && judgement.trees.is_empty() {
            println!("  no trees");
        }
        if dead {
            burns += 1;
        } else if cutting {
            cuts += removals.len();
        }
        if !args.yes {
            continue;
        }
        let done = if dead {
            burn_forest(config, &judgement.forest, &removals)
        } else {
            removals
                .iter()
                .try_for_each(|(tree, removal)| cut_tree(&judgement.forest, tree, removal))
        };
        if let Err(err) = done {
            eprintln!("workforest: {name}: {err}");
            failures += 1;
        }
    }
    if in_flight > 0 {
        println!("{in_flight} other forest(s) in flight");
    }
    if kept_with_dead > 0 {
        println!(
            "{kept_with_dead} forest(s) kept for their live trees hold dead ones; --scorch cuts those"
        );
    }
    if burns == 0 && cuts == 0 {
        println!("nothing to burn");
    } else if !args.yes {
        let mut plan = Vec::new();
        if burns > 0 {
            plan.push(format!("burn {burns} forest(s)"));
        }
        if cuts > 0 {
            plan.push(format!("cut {cuts} tree(s)"));
        }
        println!("dry run: pass --yes to {}", plan.join(" and "));
    }
    if failures > 0 {
        bail!("could not finish with {failures} forest(s)");
    }
    Ok(())
}

fn path(config: &Config, args: ForestArg) -> Result<()> {
    let forest = Forest::resolve(config, args.forest.as_deref(), NAME_AS_ARGUMENT)?;
    println!("{}", forest.dir.display());
    Ok(())
}

fn cache_status(config: &Config, args: ForestArg) -> Result<()> {
    let forest = Forest::resolve(config, args.forest.as_deref(), NAME_AS_ARGUMENT)?;
    println!("{}  {}", forest.name, forest.dir.display());
    for tree in forest.trees()? {
        let dir = forest.tree_dir(&tree.repo);
        if !dir.is_dir() {
            println!("  {:<24} MISSING", tree.repo);
            continue;
        }
        let declared = cache::declared(&tree.source);
        cache::warn(&declared.warnings);
        if declared.entries.is_empty() {
            println!("  {:<24} no caches declared", tree.repo);
            continue;
        }
        println!("  {}", tree.repo);
        for entry in &declared.entries {
            println!("    {:<22} {}", entry.path, cache::describe(&dir, entry));
        }
    }
    Ok(())
}

fn cache_graft(config: &Config, args: CacheGraftArgs) -> Result<()> {
    let forest = Forest::resolve(config, args.target.forest.as_deref(), NAME_WITH_FLAG)?;
    let link_min = config::link_min()?;
    for tree in selected_trees(&forest, &args.trees)? {
        let dir = forest.tree_dir(&tree.repo);
        if !dir.is_dir() {
            println!("{}: MISSING", tree.repo);
            continue;
        }
        println!("{}", tree.repo);
        cache::graft_tree(&dir, &tree.source, link_min, args.force);
    }
    Ok(())
}

fn cache_drop(config: &Config, args: CacheDropArgs) -> Result<()> {
    let forest = Forest::resolve(config, args.target.forest.as_deref(), NAME_WITH_FLAG)?;
    for tree in selected_trees(&forest, &args.trees)? {
        let dir = forest.tree_dir(&tree.repo);
        let declared = cache::declared(&tree.source);
        cache::warn(&declared.warnings);
        for entry in &declared.entries {
            let path = dir.join(&entry.path);
            if entry.mode != cache::Mode::Clone || path.is_symlink() || !path.is_dir() {
                continue;
            }
            match cache::remove(&dir, &entry.path) {
                Ok(()) => println!("dropped {}/{}", tree.repo, entry.path),
                Err(err) => eprintln!("workforest: {}/{}: {err}", tree.repo, entry.path),
            }
        }
    }
    Ok(())
}

fn cache_paths(args: CachePathsArgs) -> Result<()> {
    let source = git::main_worktree(&args.repo)?;
    let declared = cache::declared(&source);
    cache::warn(&declared.warnings);
    println!("{}  {}", dir_name(&source), source.display());
    if declared.entries.is_empty() {
        println!("  no caches declared");
    }
    for entry in &declared.entries {
        let globs = if entry.always_copy.is_empty() {
            "-".to_owned()
        } else {
            entry.always_copy.join(",")
        };
        println!(
            "  {:<22} {:<6} {:<34} {}",
            entry.path, entry.mode, globs, entry.origin
        );
    }
    Ok(())
}

fn cache_doctor(config: &Config, args: CacheDoctorArgs) -> Result<()> {
    let source = git::main_worktree(&args.repo)?;
    cache::doctor::doctor(config, &source, args.cmd.as_deref())
}

/// The trees in `forest` named by `names`, else all of them.
fn selected_trees(forest: &Forest, names: &[String]) -> Result<Vec<Tree>> {
    if names.is_empty() {
        return forest.trees();
    }
    names
        .iter()
        .map(|arg| {
            let repo = dir_name(Path::new(arg));
            match forest.tree(&repo)? {
                Some(tree) => Ok(tree),
                None => bail!("{repo} is not a tree in {}", forest.name),
            }
        })
        .collect()
}

fn count_or_unknown(count: Option<u64>) -> String {
    count.map_or_else(|| "?".to_owned(), |count| count.to_string())
}
