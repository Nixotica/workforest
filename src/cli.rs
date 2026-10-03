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

Examples:
  workforest new fix-login .                       one tree: the repo you're in
  workforest new auth-migration ~/code/api ~/code/web
  cd \"$(workforest path auth-migration)\"
  workforest plant ~/code/docs -B origin/release   a third tree, off another base
  workforest burn auth-migration --delete-branches";

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
    /// List forests, or the trees in one
    #[command(visible_alias = "list")]
    Ls(LsArgs),
    /// Show each tree's branch, whether it is dirty, and how far it is from its base
    #[command(visible_alias = "st")]
    Status(ForestArg),
    /// Print a forest's path
    #[command(visible_alias = "dir")]
    Path(ForestArg),
}

#[derive(Args)]
pub struct NewArgs {
    /// Name of the forest, which is also the default branch name
    pub forest: String,
    /// Paths of repos to plant right away
    pub repos: Vec<String>,
    #[command(flatten)]
    pub branching: Branching,
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

/// How far removing a tree may go.
#[derive(Args)]
pub struct Removal {
    /// Remove trees even if that loses uncommitted changes, or commits that are
    /// neither pushed nor landed on their base
    #[arg(long)]
    pub force: bool,
    /// Also delete the trees' branches from their repos
    #[arg(long)]
    pub delete_branches: bool,
}
