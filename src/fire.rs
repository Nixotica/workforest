//! Dead trees: trees whose work is done, so that burning them loses nothing.
//!
//! A tree is dead when `burn` would accept it and its work has landed on its
//! base, or when its directory is gone. A forest is dead when every tree in it
//! is dead, and nothing else in it would be lost.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;

use crate::error::Result;
use crate::forest::{Forest, Tree};
use crate::git;

/// How many repos are fetched at once.
const FETCHES_AT_ONCE: usize = 8;

/// What `fire` makes of a tree, and why.
pub enum Verdict {
    /// Its work is done, and burning it loses nothing.
    Dead(String),
    /// Work in flight, or not begun.
    Live(String),
    /// Git can't say enough to judge it, so it is left alone.
    Unknown(String),
}

impl Verdict {
    pub fn is_dead(&self) -> bool {
        matches!(self, Verdict::Dead(_))
    }

    pub fn is_live(&self) -> bool {
        matches!(self, Verdict::Live(_))
    }

    pub fn label(&self) -> &'static str {
        match self {
            Verdict::Dead(_) => "dead",
            Verdict::Live(_) => "live",
            Verdict::Unknown(_) => "?",
        }
    }

    pub fn reason(&self) -> &str {
        match self {
            Verdict::Dead(reason) | Verdict::Live(reason) | Verdict::Unknown(reason) => reason,
        }
    }
}

/// A forest and what `fire` makes of each tree in it.
pub struct Judgement {
    pub forest: Forest,
    pub trees: Vec<(Tree, Verdict)>,
    /// Checkouts in the forest that its manifest doesn't record, which `burn`
    /// refuses to delete.
    pub strays: Vec<String>,
}

impl Judgement {
    /// Whether every tree in the forest is dead, and nothing else in it would be
    /// lost. A forest with no trees left is dead.
    pub fn is_dead(&self) -> bool {
        self.strays.is_empty() && self.trees.iter().all(|(_, verdict)| verdict.is_dead())
    }

    pub fn dead_trees(&self) -> impl Iterator<Item = &Tree> {
        self.trees
            .iter()
            .filter(|(_, verdict)| verdict.is_dead())
            .map(|(tree, _)| tree)
    }

    /// Whether the forest is plainly in flight: no tree is dead, and every tree
    /// could be judged.
    pub fn is_in_flight(&self) -> bool {
        !self.trees.is_empty()
            && self.strays.is_empty()
            && self.trees.iter().all(|(_, verdict)| verdict.is_live())
    }
}

/// Judge every tree in `forest`.
pub fn judge_forest(forest: Forest) -> Result<Judgement> {
    let trees = forest.trees()?;
    let strays = forest.unrecorded_checkouts(&trees)?;
    let trees = trees
        .into_iter()
        .map(|tree| {
            let verdict = judge(&forest, &tree);
            (tree, verdict)
        })
        .collect();
    Ok(Judgement {
        forest,
        trees,
        strays,
    })
}

/// Judge one tree of `forest` against its base as last fetched.
pub fn judge(forest: &Forest, tree: &Tree) -> Verdict {
    let dir = forest.tree_dir(&tree.repo);
    if !dir.is_dir() {
        return Verdict::Dead("missing".to_owned());
    }
    if !tree.source.is_dir() {
        return Verdict::Unknown(format!("its repo {} is gone", tree.source.display()));
    }
    let unreadable = || {
        Verdict::Unknown(
            "git can't read it: its repo may no longer list it as a worktree".to_owned(),
        )
    };
    let Some(record) = git::worktree_record(&dir) else {
        return unreadable();
    };
    // A record git has since given to another worktree of the same name would
    // have this tree judged as that one.
    if !points_at(&record, &dir) {
        return Verdict::Unknown(
            "its repo's record of it belongs to another worktree: `git worktree repair` it"
                .to_owned(),
        );
    }
    if record.join("locked").exists() {
        return Verdict::Unknown(
            "it is locked: `git worktree unlock` it to let it burn".to_owned(),
        );
    }
    match git::is_dirty(&dir) {
        Some(false) => {}
        Some(true) => return Verdict::Live("uncommitted changes".to_owned()),
        None => return unreadable(),
    }
    let Some(branch) = git::current_branch(&dir) else {
        return Verdict::Unknown("its HEAD is detached".to_owned());
    };
    if branch != tree.branch {
        return Verdict::Unknown(format!(
            "it has {branch} checked out, not its branch {}",
            tree.branch
        ));
    }
    let verdict = if let Some(target) = landed_on(&dir, tree, Some(&branch)) {
        Verdict::Dead(format!("landed on {target}"))
    } else if !git::resolves(&dir, &tree.base) {
        return Verdict::Unknown(format!("its base {} no longer exists", tree.base));
    } else {
        let base = git::landing_base(&tree.source, &tree.base, &branch);
        match git::count(&dir, &format!("{base}..HEAD")) {
            None => return Verdict::Unknown(format!("it can't be compared with {base}")),
            Some(0) => Verdict::Live("nothing committed yet".to_owned()),
            Some(ahead) => Verdict::Live(format!("{ahead} commit(s) not on {base}")),
        }
    };
    // Landed work passes burn's checks by definition; this keeps it so.
    if verdict.is_dead()
        && let Some(risk) = git::unlanded_work(&dir, &tree.base)
    {
        return Verdict::Unknown(format!("burn would refuse it: {risk}"));
    }
    verdict
}

/// Where `tree`, checked out at `dir` on `branch`, has landed its work, if it
/// has: the base it lands on (see [`git::landing_base`]), or the default branch
/// standing in for a spent base (see [`git::base_stand_in`]).
pub fn landed_on(dir: &Path, tree: &Tree, branch: Option<&str>) -> Option<String> {
    let base = branch.map_or_else(
        || tree.base.clone(),
        |branch| git::landing_base(&tree.source, &tree.base, branch),
    );
    let stand_in = git::base_stand_in(dir, &base);
    [Some(base), stand_in]
        .into_iter()
        .flatten()
        .find(|target| git::work_landed(dir, target, "HEAD", branch))
}

/// Whether the worktree `record` in a repo points back at the worktree at `dir`.
fn points_at(record: &Path, dir: &Path) -> bool {
    let Ok(gitdir) = fs::read_to_string(record.join("gitdir")) else {
        return false;
    };
    match (
        fs::canonicalize(gitdir.trim_end_matches('\n')),
        fs::canonicalize(dir.join(".git")),
    ) {
        (Ok(points_to), Ok(dot_git)) => points_to == dot_git,
        _ => false,
    }
}

/// Whether deleting `tree`'s branch would lose no commits: everything on it is
/// on its base, or on the default branch standing in for a base that is gone,
/// or there is no such branch.
pub fn branch_spent(tree: &Tree) -> bool {
    if !git::branch_exists(&tree.source, &tree.branch) {
        return true;
    }
    let tip = format!("refs/heads/{}", tree.branch);
    let on = |target: &str| match git::count(&tree.source, &format!("{target}..{tip}")) {
        Some(0) => true,
        Some(_) => git::landed(&tree.source, target, &tip),
        None => false,
    };
    on(&tree.base)
        || git::base_stand_in(&tree.source, &tree.base).is_some_and(|stand_in| on(&stand_in))
}

/// Fetch the remotes that the trees' bases are on, once per repo, several repos
/// at a time, pruning deleted branches so that a base that is gone looks gone. A repo that can't be fetched is reported and judged as last
/// fetched; a forest whose manifest can't be read is reported when it is judged.
pub fn fetch_bases(forests: &[Forest]) {
    let mut bases: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
    for forest in forests {
        for tree in forest.trees().unwrap_or_default() {
            if tree.source.is_dir() {
                bases.entry(tree.source).or_default().insert(tree.base);
            }
        }
    }
    let queue = Mutex::new(bases.into_iter());
    thread::scope(|scope| {
        for _ in 0..FETCHES_AT_ONCE {
            scope.spawn(|| {
                loop {
                    let next = queue.lock().ok().and_then(|mut queue| queue.next());
                    let Some((repo, bases)) = next else {
                        break;
                    };
                    let remotes: BTreeSet<String> = bases
                        .iter()
                        .filter_map(|base| git::remote_of(&repo, base))
                        .collect();
                    for remote in remotes {
                        if !git::fetch(&repo, &remote) {
                            eprintln!(
                                "workforest: could not fetch {remote} into {}; judging it as last fetched",
                                repo.display()
                            );
                        }
                    }
                }
            });
        }
    });
}
