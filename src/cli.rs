//! The command-line interface.

use clap::{Args, Parser, Subcommand, ValueEnum};
use clap_complete::engine::ArgValueCompleter;

use crate::complete;

/// The version `--version` reports. A Nix build passes the version with its
/// commit in `WORKFOREST_VERSION`; any other build reports the crate version.
pub const VERSION: &str = match option_env!("WORKFOREST_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

const AFTER_HELP: &str = "\
Repos are given as paths, absolute or relative to the current directory: `.` is
the repo you're in. Once `workforest setup` has said where your repos live, a
bare name such as api names one there. Forests live under $WORKFOREST_ROOT, else forest_root in
~/.config/workforest/config.toml, else ~/.workforest; see `workforest config`.

Planting a tree also grafts the build caches its repo declares in
.workforest-cache; see `workforest help cache`.

Examples:
  workforest new fix-login .                       one tree: the repo you're in
  workforest new auth-migration ~/code/api ~/code/web
  cd \"$(workforest path auth-migration)\"
  workforest plant ~/code/docs -B origin/release   a third tree, off another base
  workforest burn auth-migration --delete-branches
  workforest fire                                  which forests have landed, and why";

/// One git worktree per repo in a piece of work, isolated from the main
/// checkouts. A forest with a single tree is normal; several trees share one
/// branch name.
#[derive(Parser)]
#[command(name = "workforest", version = VERSION, after_help = AFTER_HELP)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Create a forest, optionally planting trees in it
    New(NewArgs),
    /// Plant trees: add a worktree of each repo to a forest
    #[command(visible_alias = "add")]
    Plant(PlantArgs),
    /// Cut trees: remove their worktrees from a forest
    #[command(visible_alias = "remove")]
    Cut(CutArgs),
    /// Burn a forest: remove it and every tree in it
    #[command(visible_aliases = ["rm", "delete"])]
    Burn(BurnArgs),
    /// Burn every forest whose work has landed; a dry run unless --yes
    #[command(after_help = FIRE_AFTER_HELP)]
    Fire(FireArgs),
    /// List forests, or the trees in one
    #[command(visible_alias = "list")]
    Ls(LsArgs),
    /// Show each tree's branch, whether it is dirty or pushed, and how far it is from its base
    #[command(visible_alias = "st")]
    Status(ReportArgs),
    /// Print a forest's path
    #[command(visible_alias = "dir")]
    Path(ForestArg),
    /// Run a command in every tree of a forest: `workforest exec -- cargo test`
    Exec(ExecArgs),
    /// Manage the build caches grafted into trees
    Cache(CacheArgs),
    /// Show each setting, its value, and where it comes from
    Config,
    /// Say where your repos live, so that they can be named rather than given
    /// as paths
    Setup(SetupArgs),
    /// Print a shell's completion script, for workforest and wf
    Completions(CompletionsArgs),
    /// Print shell functions to load at startup: wfcd, which cds into a forest
    /// or one of its trees
    ShellInit(ShellArg),
    /// Print the names of every forest, or of the trees in one, for shell
    /// functions to complete
    #[command(name = "__names", hide = true)]
    Names(NamesArgs),
}

#[derive(Args)]
pub struct NewArgs {
    /// Name of the forest, which is also the default branch name
    pub forest: String,
    /// Repos to plant right away, by path or by name
    #[arg(add = ArgValueCompleter::new(complete::repos))]
    pub repos: Vec<String>,
    #[command(flatten)]
    pub branching: Branching,
    #[command(flatten)]
    pub sparse: Sparse,
    #[command(flatten)]
    pub caching: Caching,
}

#[derive(Args)]
pub struct PlantArgs {
    /// Repos to plant, by path or by name
    #[arg(required = true, add = ArgValueCompleter::new(complete::repos))]
    pub repos: Vec<String>,
    #[command(flatten)]
    pub target: Target,
    #[command(flatten)]
    pub branching: Branching,
    #[command(flatten)]
    pub sparse: Sparse,
    #[command(flatten)]
    pub caching: Caching,
}

#[derive(Args)]
pub struct CutArgs {
    /// Trees to cut, by name
    #[arg(required = true, add = ArgValueCompleter::new(complete::trees))]
    pub trees: Vec<String>,
    #[command(flatten)]
    pub target: Target,
    #[command(flatten)]
    pub removal: Removal,
}

#[derive(Args)]
pub struct BurnArgs {
    /// Forest to burn [default: the forest containing the current directory]
    #[arg(add = ArgValueCompleter::new(complete::forests))]
    pub forest: Option<String>,
    #[command(flatten)]
    pub removal: Removal,
}

const FIRE_AFTER_HELP: &str = "\
A tree is dead when burn would accept it and its work has landed: its branch
has commits of its own, and everything they changed is on its base, however
they were merged. Once a base branch is gone, as a stacked branch's base is
after it merges, the repo's default branch stands in for it. A tree whose
directory is gone is dead too. A tree with uncommitted changes, with commits
not on its base, or with nothing committed yet is live; one git can't judge is
left alone.

A forest burns when every tree in it is dead. fire never removes anything burn
would refuse, and has no --force.

Before judging, fire fetches the remotes the trees' bases are on, pruning the
branches deleted from them, so that merges are seen and deleted bases look
gone.";

#[derive(Args)]
pub struct FireArgs {
    /// Burn the dead forests; without this, only list them and why
    #[arg(short, long)]
    pub yes: bool,
    #[command(flatten)]
    pub output: Output,
    /// Also cut dead trees out of forests that still have live ones
    #[arg(long)]
    pub scorch: bool,
    /// Judge the bases as last fetched, without fetching them first
    #[arg(long)]
    pub no_fetch: bool,
    /// Also delete the dead trees' branches from their repos, unless a branch
    /// has commits not on its base
    #[arg(long)]
    pub delete_branches: bool,
}

#[derive(Args)]
pub struct ExecArgs {
    #[command(flatten)]
    pub target: Target,
    /// Run in up to N trees at once, printing each tree's output as it
    /// finishes [default N: one per CPU]
    #[arg(long, value_name = "N", num_args = 0..=1, default_missing_value = "0")]
    pub parallel: Option<usize>,
    /// The command and its arguments, after `--`. It runs as given, not
    /// through a shell, with WORKFOREST_FOREST and WORKFOREST_TREE set
    #[arg(last = true, required = true, value_name = "COMMAND")]
    pub command: Vec<std::ffi::OsString>,
}

#[derive(Args)]
pub struct CompletionsArgs {
    pub shell: Shell,
    /// The command to complete [default: workforest and wf]
    #[arg(long, value_parser = ["workforest", "wf"])]
    pub bin: Option<String>,
}

#[derive(Args)]
pub struct ShellArg {
    pub shell: Shell,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
}

#[derive(Args)]
pub struct NamesArgs {
    /// Forest whose trees to name [default: name every forest]
    pub forest: Option<String>,
}

#[derive(Args)]
pub struct SetupArgs {
    /// Directories holding your repos, written to the config file without
    /// asking [default: ask, suggesting directories that hold several repos]
    #[arg(long, value_name = "DIR", num_args = 1..)]
    pub repos: Vec<String>,
}

#[derive(Args)]
pub struct LsArgs {
    /// Forest whose trees to list [default: list every forest]
    #[arg(add = ArgValueCompleter::new(complete::forests))]
    pub forest: Option<String>,
    #[command(flatten)]
    pub output: Output,
}

/// A forest to report on, and how.
#[derive(Args)]
pub struct ReportArgs {
    /// Forest to act on [default: the forest containing the current directory]
    #[arg(add = ArgValueCompleter::new(complete::forests))]
    pub forest: Option<String>,
    #[command(flatten)]
    pub output: Output,
}

/// How a report is printed.
#[derive(Args)]
pub struct Output {
    /// Print JSON for scripts and agents; see "JSON output" in the README
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct ForestArg {
    /// Forest to act on [default: the forest containing the current directory]
    #[arg(add = ArgValueCompleter::new(complete::forests))]
    pub forest: Option<String>,
}

/// Which forest a command acts on.
#[derive(Args)]
pub struct Target {
    /// Forest to act on [default: the forest containing the current directory]
    #[arg(short, long, add = ArgValueCompleter::new(complete::forests))]
    pub forest: Option<String>,
}

/// Which part of its repo a planted tree checks out.
#[derive(Args)]
pub struct Sparse {
    /// Check out only these directories of each repo, plus its top-level
    /// files, as a sparse checkout of the tree's own
    #[arg(long, value_name = "DIR", num_args = 1..)]
    pub sparse: Vec<String>,
}

/// Which branch a planted tree gets.
#[derive(Args)]
pub struct Branching {
    /// Branch to check out, created if it does not exist [default: the forest name]
    #[arg(short, long)]
    pub branch: Option<String>,
    /// Ref to create the branch from [default: origin/HEAD, else main or master]
    #[arg(short = 'B', long)]
    pub base: Option<String>,
}

/// Whether planted trees get their repos' build caches.
#[derive(Args)]
pub struct Caching {
    /// Don't graft the repos' build caches into the new trees
    #[arg(long)]
    pub no_cache: bool,
}

/// How far removing a tree may go.
#[derive(Args, Clone, Copy)]
pub struct Removal {
    /// Remove trees even if that loses uncommitted changes, or commits that are
    /// neither pushed nor landed on their base
    #[arg(long)]
    pub force: bool,
    /// Also delete the trees' branches from their repos
    #[arg(long)]
    pub delete_branches: bool,
}

const CACHE_AFTER_HELP: &str = "\
A repo declares its cache directories in .workforest-cache at its root, or in
workforest-cache in its git common dir, which is machine-local and overrides
it. One cache per line: its path, a mode, and the globs of files to copy
whatever their size, comma-separated, or - for none. # starts a comment:

  # path   mode    always-copy globs
  build    clone   *.lock,state/*
  .venv    never

  clone   graft the main checkout's directory: files of 64 KiB and up are
          hardlinked, smaller ones copied (the default)
  never   leave it cold

workforest only replaces or deletes cache paths that git ignores. Before
trusting a clone entry, test it with `workforest cache doctor`.

The size from which files are hardlinked is $WORKFOREST_CACHE_LINK_MIN, else
cache.link_min in the config file, else 65536.";

/// Build caches, grafted from each repo's main checkout so a new tree doesn't
/// build from cold.
#[derive(Args)]
#[command(after_help = CACHE_AFTER_HELP)]
pub struct CacheArgs {
    #[command(subcommand)]
    pub command: CacheCommand,
}

#[derive(Subcommand)]
pub enum CacheCommand {
    /// Show how much of each tree's caches is still hardlinked to the main checkout
    #[command(visible_alias = "st")]
    Status(ReportArgs),
    /// Graft caches into trees that are already planted
    Graft(CacheGraftArgs),
    /// Delete trees' grafted clone caches
    Drop(CacheDropArgs),
    /// Show the caches a repo declares, and where each declaration comes from
    Paths(CachePathsArgs),
    /// Test a repo's clone caches: graft them into a throwaway worktree, build
    /// there, and check that the build wrote nothing through to the main checkout
    Doctor(CacheDoctorArgs),
}

#[derive(Args)]
pub struct CacheGraftArgs {
    /// Trees to graft into, by name [default: every tree in the forest]
    #[arg(add = ArgValueCompleter::new(complete::trees))]
    pub trees: Vec<String>,
    #[command(flatten)]
    pub target: Target,
    /// Replace caches the trees already have
    #[arg(long)]
    pub force: bool,
}

#[derive(Args)]
pub struct CacheDropArgs {
    /// Trees whose caches to drop, by name [default: every tree in the forest]
    #[arg(add = ArgValueCompleter::new(complete::trees))]
    pub trees: Vec<String>,
    #[command(flatten)]
    pub target: Target,
}

#[derive(Args)]
pub struct CachePathsArgs {
    /// The repo, by path or by name
    #[arg(add = ArgValueCompleter::new(complete::repos))]
    pub repo: String,
}

#[derive(Args)]
pub struct CacheDoctorArgs {
    /// The repo, by path or by name
    #[arg(add = ArgValueCompleter::new(complete::repos))]
    pub repo: String,
    /// Shell command that builds the repo, run in the throwaway worktree
    /// [default: `cargo build --all-targets && cargo doc` in a repo with a
    /// root Cargo.toml]
    #[arg(long)]
    pub cmd: Option<String>,
}
