//! The command-line interface.

use clap::{Args, Parser, Subcommand};

/// The version `--version` reports. A Nix build passes the version with its
/// commit in `WORKFOREST_VERSION`; any other build reports the crate version.
pub const VERSION: &str = match option_env!("WORKFOREST_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

const AFTER_HELP: &str = "\
Repos are given as paths, absolute or relative to the current directory: `.` is
the repo you're in. Forests live under $WORKFOREST_ROOT (default ~/.workforest).

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
    Status(ForestArg),
    /// Print a forest's path
    #[command(visible_alias = "dir")]
    Path(ForestArg),
    /// Manage the build caches grafted into trees
    Cache(CacheArgs),
}

#[derive(Args)]
pub struct NewArgs {
    /// Name of the forest, which is also the default branch name
    pub forest: String,
    /// Paths of repos to plant right away
    pub repos: Vec<String>,
    #[command(flatten)]
    pub branching: Branching,
    #[command(flatten)]
    pub caching: Caching,
}

#[derive(Args)]
pub struct PlantArgs {
    /// Paths of repos to plant
    #[arg(required = true)]
    pub repos: Vec<String>,
    #[command(flatten)]
    pub target: Target,
    #[command(flatten)]
    pub branching: Branching,
    #[command(flatten)]
    pub caching: Caching,
}

#[derive(Args)]
pub struct CutArgs {
    /// Trees to cut, by name
    #[arg(required = true)]
    pub trees: Vec<String>,
    #[command(flatten)]
    pub target: Target,
    #[command(flatten)]
    pub removal: Removal,
}

#[derive(Args)]
pub struct BurnArgs {
    /// Forest to burn [default: the forest containing the current directory]
    pub forest: Option<String>,
    #[command(flatten)]
    pub removal: Removal,
}

const FIRE_AFTER_HELP: &str = "\
A tree is dead when burn would accept it and its work has landed: its branch
has commits of its own, and everything they changed is on its base, however
they were merged. A tree whose directory is gone is dead too. A tree with
uncommitted changes, with commits not on its base, or with nothing committed
yet is live; one git can't judge is left alone.

A forest burns when every tree in it is dead. fire never removes anything burn
would refuse, and has no --force.

Before judging, fire fetches the remotes the trees' bases are on, so their
merges are seen.";

#[derive(Args)]
pub struct FireArgs {
    /// Burn the dead forests; without this, only list them and why
    #[arg(short, long)]
    pub yes: bool,
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
pub struct LsArgs {
    /// Forest whose trees to list [default: list every forest]
    pub forest: Option<String>,
}

#[derive(Args)]
pub struct ForestArg {
    /// Forest to act on [default: the forest containing the current directory]
    pub forest: Option<String>,
}

/// Which forest a command acts on.
#[derive(Args)]
pub struct Target {
    /// Forest to act on [default: the forest containing the current directory]
    #[arg(short, long)]
    pub forest: Option<String>,
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

Environment: WORKFOREST_CACHE_LINK_MIN (default 65536).";

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
    Status(ForestArg),
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
    pub trees: Vec<String>,
    #[command(flatten)]
    pub target: Target,
}

#[derive(Args)]
pub struct CachePathsArgs {
    /// Path of the repo
    pub repo: String,
}

#[derive(Args)]
pub struct CacheDoctorArgs {
    /// Path of the repo
    pub repo: String,
    /// Shell command that builds the repo, run in the throwaway worktree
    /// [default: `cargo build --all-targets && cargo doc` in a repo with a
    /// root Cargo.toml]
    #[arg(long)]
    pub cmd: Option<String>,
}
