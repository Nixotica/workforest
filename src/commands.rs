//! What each subcommand does.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use clap::CommandFactory;
use serde_json::{Value, json};

use crate::cache;
use crate::cli::{
    Branching, BurnArgs, CacheCommand, CacheDoctorArgs, CacheDonateArgs, CacheDropArgs,
    CacheGraftArgs, CachePathsArgs, Caching, Cli, Command, CutArgs, FireArgs, ForestArg, LsArgs,
    NewArgs, PlantArgs, Removal, ReportArgs, SetupArgs,
};
use crate::complete;
use crate::config::{self, Config};
use crate::error::{Context, Result, bail};
use crate::exec;
use crate::fire;
use crate::forest::{Forest, Tree, cwd_is_within, dir_name};
use crate::git;
use crate::repos;

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
        Command::New(args) => new(&Config::load()?, args)?,
        Command::Plant(args) => plant(&Config::load()?, args)?,
        Command::Cut(args) => cut(&Config::load()?, args)?,
        Command::Burn(args) => burn(&Config::load()?, args)?,
        Command::Fire(args) => fire(&Config::load()?, args)?,
        Command::Ls(args) => ls(&Config::load()?, args)?,
        Command::Status(args) => status(&Config::load()?, args)?,
        Command::Path(args) => path(&Config::load()?, args)?,
        Command::Exec(args) => {
            let config = Config::load()?;
            let forest = Forest::resolve(&config, args.target.forest.as_deref(), NAME_WITH_FLAG)?;
            let at_once = args.parallel.map(|n| if n == 0 { exec::cpus() } else { n });
            exec::exec(&forest, &args.command, at_once)?;
        }
        Command::Config => show_config(&Config::load()?),
        Command::Setup(args) => setup(&Config::load()?, args)?,
        Command::Completions(args) => complete::print_registration(&args)?,
        Command::ShellInit(args) => complete::print_shell_init(args.shell)?,
        Command::Names(args) => complete::print_names(&args)?,
        Command::Cache(args) => {
            let config = Config::load()?;
            match args.command {
                CacheCommand::Status(args) => cache_status(&config, args)?,
                CacheCommand::Graft(args) => cache_graft(&config, args)?,
                CacheCommand::Drop(args) => cache_drop(&config, args)?,
                CacheCommand::Donate(args) => cache_donate(&config, args)?,
                CacheCommand::Paths(args) => cache_paths(&config, args)?,
                CacheCommand::Doctor(args) => cache_doctor(&config, args)?,
            }
        }
    }
    Ok(())
}

fn new(config: &Config, args: NewArgs) -> Result<()> {
    let sources = main_worktrees(config, &args.repos)?;
    check_sparse(&args.sparse.sparse)?;
    let graft = grafting(config, &args.caching)?;
    let forest = Forest::create(config, &args.forest)?;
    println!("new forest {} at {}", forest.name, forest.dir.display());
    if sources.is_empty() {
        return Ok(());
    }
    plant_sources(&forest, sources, args.branching, &args.sparse.sparse, graft)
}

fn plant(config: &Config, args: PlantArgs) -> Result<()> {
    let forest = Forest::resolve(config, args.target.forest.as_deref(), NAME_WITH_FLAG)?;
    let sources = main_worktrees(config, &args.repos)?;
    check_sparse(&args.sparse.sparse)?;
    let graft = grafting(config, &args.caching)?;
    plant_sources(&forest, sources, args.branching, &args.sparse.sparse, graft)
}

/// `--sparse` takes directories inside the repo: relative, and not leaving it.
fn check_sparse(dirs: &[String]) -> Result<()> {
    for dir in dirs {
        let path = Path::new(dir);
        let leaves = path
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)));
        if dir.is_empty() || dir.starts_with('-') || leaves {
            bail!("--sparse takes directories inside the repo, such as src/api, not '{dir}'");
        }
    }
    Ok(())
}

/// The size from which grafted cache files are hardlinked, or `None` if
/// `--no-cache` says not to graft.
fn grafting(config: &Config, caching: &Caching) -> Result<Option<u64>> {
    if caching.no_cache {
        return Ok(None);
    }
    Ok(Some(config.link_min()?.value))
}

/// The main worktree of each repo argument, all checked before anything changes.
fn main_worktrees(config: &Config, repos: &[String]) -> Result<Vec<PathBuf>> {
    repos.iter().map(|arg| main_worktree(config, arg)).collect()
}

/// The main worktree of the repo that `arg` names, by path or by name.
fn main_worktree(config: &Config, arg: &str) -> Result<PathBuf> {
    git::main_worktree(&repos::resolve(config, arg)?)
}

/// Add a worktree of each repo to `forest`, all on the same branch. Unless
/// `graft` is `None`, graft the build caches each repo declares, hardlinking
/// files of that many bytes and up.
fn plant_sources(
    forest: &Forest,
    sources: Vec<PathBuf>,
    branching: Branching,
    sparse: &[String],
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
        git::add_worktree(&source, &dir, &branch, &base, sparse)?;
        forest.record(Tree {
            repo: repo.clone(),
            source: source.clone(),
            branch: branch.clone(),
            base: base.clone(),
        })?;
        let only = if sparse.is_empty() {
            String::new()
        } else {
            format!(", only {}", sparse.join(" "))
        };
        println!(
            "planted {repo} -> {} (branch {branch}, off {base}{only})",
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
        cut_tree(&forest, &tree, &args.removal, false)?;
    }
    Ok(())
}

/// Remove one tree from `forest`, saying so unless `quiet`, and saying where to
/// go if the shell was in it.
fn cut_tree(forest: &Forest, tree: &Tree, removal: &Removal, quiet: bool) -> Result<()> {
    let dir = forest.tree_dir(&tree.repo);
    let standing_in_it = cwd_is_within(&dir);
    remove_tree(forest, tree, removal)?;
    if !quiet {
        println!("cut {} from {}", tree.repo, forest.name);
    }
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
    burn_forest(config, &forest, &removals, false)
}

/// Remove each tree of `forest` as its removal says, then the forest itself,
/// saying so unless `quiet`, and saying where to go if the shell was in it.
fn burn_forest(
    config: &Config,
    forest: &Forest,
    trees: &[(Tree, Removal)],
    quiet: bool,
) -> Result<()> {
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
    if !quiet {
        println!("burned forest {}", forest.name);
    }
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

/// Remove one tree's worktree, donating its caches and deleting its branch if
/// asked, then forget it. A tree whose directory and repo are both gone only
/// needs forgetting.
fn remove_tree(forest: &Forest, tree: &Tree, removal: &Removal) -> Result<()> {
    let dir = forest.tree_dir(&tree.repo);
    if removal.donate_cache && dir.is_dir() && tree.source.is_dir() {
        let header = format!("{}: donating caches", tree.repo);
        cache::donate_tree(
            &dir,
            &tree.source,
            cache::Transfer::Move,
            false,
            Some(&header),
        );
    }
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
    if args.output.json {
        let forests = match &args.forest {
            Some(name) => vec![Forest::named(config, name)?],
            None => Forest::all(config)?,
        };
        let mut listed = Vec::new();
        for forest in &forests {
            let trees: Vec<Value> = forest
                .trees()?
                .iter()
                .map(|tree| tree_json(forest, tree))
                .collect();
            listed.push(json!({
                "name": forest.name,
                "path": path_json(&forest.dir),
                "trees": trees,
            }));
        }
        print_json(json!({
            "forest_root": path_json(&config.forest_root),
            "forests": listed,
        }));
        return Ok(());
    }
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
    /// Its directory is gone.
    Missing,
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
            Self::Missing => "MISSING",
            Self::Unknown => "?",
            Self::Dirty => "dirty",
            Self::Landed => "landed",
            Self::Clean => "clean",
        }
    }
}

/// What `status` finds out about one tree.
struct TreeStatus {
    state: TreeState,
    ahead: Option<u64>,
    behind: Option<u64>,
    /// How many commits ahead of the base its upstream lacks, all of them
    /// without an upstream. Only counted for commits ahead that haven't
    /// landed: landed work is safe to burn whether or not it was pushed.
    unpushed: Option<u64>,
    /// The directories it checks out, if it is a sparse checkout.
    sparse: Option<Vec<String>>,
}

fn tree_status(forest: &Forest, tree: &Tree) -> TreeStatus {
    let dir = forest.tree_dir(&tree.repo);
    if !dir.is_dir() {
        return TreeStatus {
            state: TreeState::Missing,
            ahead: None,
            behind: None,
            unpushed: None,
            sparse: None,
        };
    }
    let ahead = git::count(&dir, &format!("{}..HEAD", tree.base));
    let behind = git::count(&dir, &format!("HEAD..{}", tree.base));
    let state = match git::is_dirty(&dir) {
        Some(true) => TreeState::Dirty,
        Some(false) if head_landed(&dir, tree) => TreeState::Landed,
        Some(false) => TreeState::Clean,
        None => TreeState::Unknown,
    };
    let unpushed = match ahead {
        Some(ahead) if ahead > 0 && !matches!(state, TreeState::Landed) => unpushed(&dir, ahead),
        _ => None,
    };
    TreeStatus {
        state,
        ahead,
        behind,
        unpushed,
        sparse: git::sparse_dirs(&dir),
    }
}

fn status(config: &Config, args: ReportArgs) -> Result<()> {
    let forest = Forest::resolve(config, args.forest.as_deref(), NAME_AS_ARGUMENT)?;
    let trees = forest.trees()?;
    if args.output.json {
        let trees: Vec<Value> = trees
            .iter()
            .map(|tree| {
                let status = tree_status(&forest, tree);
                let mut shown = tree_json(&forest, tree);
                shown["state"] =
                    json!(status.state.as_str().to_lowercase().replace('?', "unknown"));
                shown["ahead"] = json!(status.ahead);
                shown["behind"] = json!(status.behind);
                shown["unpushed"] = json!(status.unpushed);
                shown["sparse"] = json!(status.sparse);
                shown
            })
            .collect();
        print_json(json!({
            "name": forest.name,
            "path": path_json(&forest.dir),
            "trees": trees,
        }));
        return Ok(());
    }
    println!("{}  {}", forest.name, forest.dir.display());
    for tree in &trees {
        let status = tree_status(&forest, tree);
        if let TreeState::Missing = status.state {
            println!("  {:<24} MISSING", tree.repo);
            continue;
        }
        let push = match (status.ahead, status.state) {
            (Some(ahead), state) if ahead > 0 && !matches!(state, TreeState::Landed) => {
                match status.unpushed {
                    Some(0) => ", pushed".to_owned(),
                    unpushed => format!(", {} unpushed", count_or_unknown(unpushed)),
                }
            }
            _ => String::new(),
        };
        let sparse = match &status.sparse {
            Some(dirs) => format!(", sparse: {}", dirs.join(" ")),
            None => String::new(),
        };
        println!(
            "  {:<24} {:<24} {:<6} +{}/-{} vs {}{push}{sparse}",
            tree.repo,
            tree.branch,
            status.state.as_str(),
            count_or_unknown(status.ahead),
            count_or_unknown(status.behind),
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

/// How many of the `ahead` commits of the tree at `dir` its upstream lacks. As
/// `burn` sees it, none are pushed when the branch has no upstream.
fn unpushed(dir: &Path, ahead: u64) -> Option<u64> {
    if git::has_upstream(dir) {
        git::count(dir, "@{upstream}..HEAD")
    } else {
        Some(ahead)
    }
}

/// The version of the shape of the JSON that commands print. It goes up only
/// when a change would break a reader, not when fields are added.
const JSON_SCHEMA: u64 = 1;

/// Print a JSON report: `report`, an object, with the schema version added.
fn print_json(mut report: Value) {
    report["schema"] = json!(JSON_SCHEMA);
    // A reader that stops early, such as `head`, isn't an error.
    let _ = writeln!(io::stdout().lock(), "{report:#}");
}

/// A path as JSON: a string, with any bytes that aren't UTF-8 replaced.
fn path_json(path: &Path) -> Value {
    json!(path.to_string_lossy())
}

/// A time as JSON: whole seconds since the epoch, or `null` for none.
fn time_json(time: Option<SystemTime>) -> Value {
    json!(
        time.and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|since| since.as_secs())
    )
}

/// What the manifest records about `tree`, as JSON.
fn tree_json(forest: &Forest, tree: &Tree) -> Value {
    json!({
        "repo": tree.repo,
        "path": path_json(&forest.tree_dir(&tree.repo)),
        "source": path_json(&tree.source),
        "branch": tree.branch,
        "base": tree.base,
    })
}

fn fire(config: &Config, args: FireArgs) -> Result<()> {
    let json = args.output.json;
    let forests = Forest::all(config)?;
    if !args.no_fetch {
        fire::fetch_bases(&forests);
    }
    let (mut burns, mut cuts, mut kept_with_dead, mut in_flight, mut failures) = (0, 0, 0, 0, 0);
    let mut reported = Vec::new();
    for forest in forests {
        let name = forest.name.clone();
        let judgement = match fire::judge_forest(forest) {
            Ok(judgement) => judgement,
            Err(err) => {
                eprintln!("workforest: {name}: {err}");
                reported.push(json!({"name": name, "error": err.to_string()}));
                failures += 1;
                continue;
            }
        };
        let dead = judgement.is_dead();
        let has_dead = judgement.dead_trees().next().is_some();
        let cutting = !dead && has_dead && args.scorch;
        let action = if dead {
            "burn"
        } else if cutting {
            "cut"
        } else {
            "keep"
        };
        if !dead && has_dead && !args.scorch {
            kept_with_dead += 1;
        }
        let quiet = json || judgement.is_in_flight();
        if judgement.is_in_flight() {
            in_flight += 1;
        } else if !json {
            let header = if cutting {
                "cut its dead trees"
            } else {
                action
            };
            println!("{name}  {header}");
        }
        let mut removals = Vec::new();
        let mut trees = Vec::new();
        for (tree, verdict) in &judgement.trees {
            let mut reason = verdict.reason().to_owned();
            let mut keeps_branch = false;
            if verdict.is_dead() && (dead || cutting) {
                let delete_branch = args.delete_branches && fire::branch_spent(tree);
                keeps_branch = args.delete_branches && !delete_branch;
                if keeps_branch {
                    reason.push_str(&format!(
                        "; keeps branch {}, which has commits not on {}",
                        tree.branch, tree.base
                    ));
                }
                let removal = Removal {
                    force: false,
                    delete_branches: delete_branch,
                    donate_cache: false,
                };
                removals.push((tree.clone(), removal));
            }
            if !quiet {
                println!("  {:<24} {:<6} {reason}", tree.repo, verdict.label());
            }
            trees.push(json!({
                "repo": tree.repo,
                "verdict": verdict.label().replace('?', "unknown"),
                "reason": verdict.reason(),
                "keeps_branch": keeps_branch,
            }));
        }
        if !quiet {
            for stray in &judgement.strays {
                println!(
                    "  {stray:<24} {:<6} a checkout the manifest doesn't record",
                    "?"
                );
            }
            if dead && judgement.trees.is_empty() {
                println!("  no trees");
            }
        }
        if dead {
            burns += 1;
        } else if cutting {
            cuts += removals.len();
        }
        let mut done = None;
        if args.yes && action != "keep" {
            let outcome = if dead {
                burn_forest(config, &judgement.forest, &removals, json)
            } else {
                removals.iter().try_for_each(|(tree, removal)| {
                    cut_tree(&judgement.forest, tree, removal, json)
                })
            };
            if let Err(err) = &outcome {
                eprintln!("workforest: {name}: {err}");
                failures += 1;
            }
            done = Some(outcome.map_err(|err| err.to_string()));
        }
        reported.push(json!({
            "name": name,
            "path": path_json(&judgement.forest.dir),
            "action": action,
            "trees": trees,
            "strays": judgement.strays,
            "done": done.as_ref().map(|outcome| outcome.is_ok()),
            "error": done.and_then(|outcome| outcome.err()),
        }));
    }
    if json {
        print_json(json!({"dry_run": !args.yes, "forests": reported}));
    } else {
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
    }
    if failures > 0 {
        bail!("could not finish with {failures} forest(s)");
    }
    Ok(())
}

fn setup(config: &Config, args: SetupArgs) -> Result<()> {
    let answers = if args.repos.is_empty() {
        ask_for_repos()?
    } else {
        args.repos
    };
    let mut dirs = Vec::new();
    for answer in &answers {
        let dir = config::expand_home(answer)?;
        match fs::canonicalize(&dir) {
            Ok(dir) if dir.is_dir() => dirs.push(dir),
            _ => bail!("no such directory: {}", dir.display()),
        }
    }
    config.write_repos(&dirs)?;
    for dir in &dirs {
        println!(
            "repos in {} ({} found)",
            config::tilde(dir),
            repos::count(dir)
        );
    }
    println!("written to {}", config.file().0.display());
    Ok(())
}

/// Ask on the terminal where repos live, suggesting the directories that look
/// like it. Enter takes the first suggestion.
fn ask_for_repos() -> Result<Vec<String>> {
    let suggestions = repos::suggestions(&config::home()?);
    eprintln!("Where do your repos live? Repos named on the command line are looked for there.");
    for (dir, count) in &suggestions {
        eprintln!("  {}  ({count} repos)", config::tilde(dir));
    }
    let first = suggestions.first().map(|(dir, _)| config::tilde(dir));
    match &first {
        Some(first) => eprint!("Directory [{first}]: "),
        None => eprint!("Directory: "),
    }
    io::stderr()
        .flush()
        .context("could not write to the terminal")?;
    let mut answer = String::new();
    let read = io::stdin()
        .read_line(&mut answer)
        .context("could not read an answer")?;
    if read == 0 {
        bail!("no answer: run `workforest setup --repos <dir>` to set up without asking");
    }
    match (answer.trim(), first) {
        ("", Some(first)) => Ok(vec![first]),
        ("", None) => bail!("no directory given"),
        (answer, _) => Ok(vec![answer.to_owned()]),
    }
}

fn show_config(config: &Config) {
    let (path, exists) = config.file();
    let found = if exists { "" } else { " (not found)" };
    println!("{}{found}", path.display());
    for shown in config.shown() {
        let value = shown
            .value
            .unwrap_or_else(|problem| format!("unusable: {problem}"));
        println!("  {:<16} {value:<32} {}", shown.name, shown.source);
    }
    for name in config.unknown_settings() {
        eprintln!("workforest: {}: unknown setting {name}", path.display());
    }
}

fn path(config: &Config, args: ForestArg) -> Result<()> {
    let forest = Forest::resolve(config, args.forest.as_deref(), NAME_AS_ARGUMENT)?;
    println!("{}", forest.dir.display());
    Ok(())
}

fn cache_status(config: &Config, args: ReportArgs) -> Result<()> {
    let forest = Forest::resolve(config, args.forest.as_deref(), NAME_AS_ARGUMENT)?;
    let json = args.output.json;
    if !json {
        println!("{}  {}", forest.name, forest.dir.display());
    }
    let mut trees = Vec::new();
    for tree in forest.trees()? {
        let dir = forest.tree_dir(&tree.repo);
        let mut shown = tree_json(&forest, &tree);
        if !dir.is_dir() {
            if !json {
                println!("  {:<24} MISSING", tree.repo);
            }
            shown["missing"] = json!(true);
            trees.push(shown);
            continue;
        }
        let declared = cache::declared(&tree.source);
        cache::warn(&declared.warnings);
        if !json {
            if declared.entries.is_empty() {
                println!("  {:<24} no caches declared", tree.repo);
            } else {
                println!("  {}", tree.repo);
            }
        }
        let mut caches = Vec::new();
        for entry in &declared.entries {
            if !json {
                println!(
                    "    {:<22} {}",
                    entry.path,
                    cache::describe(&dir, &tree.source, entry)
                );
                continue;
            }
            let mut cache = json!({"path": entry.path, "mode": entry.mode.to_string()});
            match cache::state(&dir, entry) {
                cache::CacheState::Never => cache["state"] = json!("never"),
                cache::CacheState::Cold => cache["state"] = json!("cold"),
                cache::CacheState::Blocked(problem) => {
                    cache["state"] = json!("blocked");
                    cache["problem"] = json!(problem);
                }
                cache::CacheState::Grafted(usage) => {
                    cache["state"] = json!("grafted");
                    cache["files"] = json!(usage.files);
                    cache["shared_bytes"] = json!(usage.shared);
                    cache["own_bytes"] = json!(usage.own);
                    cache["newest"] = time_json(usage.newest);
                    cache["main_newest"] = time_json(cache::newest(&tree.source.join(&entry.path)));
                }
            }
            caches.push(cache);
        }
        shown["missing"] = json!(false);
        shown["caches"] = json!(caches);
        trees.push(shown);
    }
    if json {
        print_json(json!({
            "name": forest.name,
            "path": path_json(&forest.dir),
            "trees": trees,
        }));
    }
    Ok(())
}

fn cache_graft(config: &Config, args: CacheGraftArgs) -> Result<()> {
    let forest = Forest::resolve(config, args.target.forest.as_deref(), NAME_WITH_FLAG)?;
    let link_min = config.link_min()?.value;
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

fn cache_donate(config: &Config, args: CacheDonateArgs) -> Result<()> {
    let forest = Forest::resolve(config, args.target.forest.as_deref(), NAME_WITH_FLAG)?;
    let link_min = config.link_min()?.value;
    for tree in selected_trees(&forest, &args.trees)? {
        let dir = forest.tree_dir(&tree.repo);
        if !dir.is_dir() {
            println!("{}: MISSING", tree.repo);
            continue;
        }
        println!("{}", tree.repo);
        let transfer = cache::Transfer::Clone { link_min };
        cache::donate_tree(&dir, &tree.source, transfer, args.force, None);
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

fn cache_paths(config: &Config, args: CachePathsArgs) -> Result<()> {
    let source = main_worktree(config, &args.repo)?;
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
    let source = main_worktree(config, &args.repo)?;
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
