//! The command-line interface.

use std::ffi::OsString;

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
  workforest graft ~/code/docs -B origin/release   a third tree, off another base
  workforest exec -- git push -u origin HEAD
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
    /// Plant a forest, optionally grafting repos into it
    #[command(visible_alias = "plant")]
    New(NewArgs),
    /// Add worktrees to a forest
    #[command(visible_alias = "add")]
    Graft(GraftArgs),
    /// Remove worktrees from a forest
    #[command(visible_alias = "remove")]
    Prune(PruneArgs),
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
    /// Run a command in every tree of a forest, one tree at a time
    #[command(visible_alias = "each")]
    Exec(ExecArgs),
}

#[derive(Args)]
pub struct NewArgs {
    /// Name of the forest, which is also the default branch name
    pub forest: String,
    /// Paths of repos to graft right away
    pub repos: Vec<String>,
    #[command(flatten)]
    pub branching: Branching,
}

#[derive(Args)]
pub struct GraftArgs {
    /// Paths of repos to graft
    #[arg(required = true)]
    pub repos: Vec<String>,
    #[command(flatten)]
    pub target: Target,
    #[command(flatten)]
    pub branching: Branching,
}

#[derive(Args)]
pub struct PruneArgs {
    /// Repos to remove from the forest
    #[arg(required = true)]
    pub repos: Vec<String>,
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

#[derive(Args)]
pub struct ExecArgs {
    #[command(flatten)]
    pub target: Target,
    /// Command to run, with its arguments
    #[arg(
        required = true,
        trailing_var_arg = true,
        allow_hyphen_values = true,
        value_name = "COMMAND"
    )]
    pub command: Vec<OsString>,
}

/// Which forest a command acts on.
#[derive(Args)]
pub struct Target {
    /// Forest to act on [default: the forest containing the current directory]
    #[arg(short, long)]
    pub forest: Option<String>,
}

/// Which branch a grafted tree gets.
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
    /// Remove trees even if that loses uncommitted changes or unpushed commits
    #[arg(long)]
    pub force: bool,
    /// Also delete the trees' branches from their repos
    #[arg(long)]
    pub delete_branches: bool,
}
