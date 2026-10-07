//! Dead trees: trees whose work is done, so that burning them loses nothing.
//!
//! A tree is dead when `burn` would accept it and the work on the branch it has
//! checked out has landed on its base, or on what stands in for that, or when
//! its directory is gone. A forest is dead when every tree in it is dead, and
//! nothing else in it would be lost.

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

    /// The same verdict, with `note` added to its reason.
    fn noting(self, note: &str) -> Verdict {
        match self {
            Verdict::Dead(reason) => Verdict::Dead(reason + note),
            Verdict::Live(reason) => Verdict::Live(reason + note),
            Verdict::Unknown(reason) => Verdict::Unknown(reason + note),
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
    let branch = git::current_branch(&dir);
    // A tree switched to another branch holds that branch's work, not its own,
    // so that is what is judged, and the reason says so.
    let switched = match &branch {
        Some(branch) if *branch != tree.branch => {
            format!(
                "; it has {branch} checked out, not its branch {}",
                tree.branch
            )
        }
        _ => String::new(),
    };
    let verdict = match git::is_dirty(&dir) {
        Some(false) => match &branch {
            Some(branch) => judge_branch(&dir, tree, branch),
            None => Verdict::Unknown("its HEAD is detached".to_owned()),
        },
        Some(true) => Verdict::Live("uncommitted changes".to_owned()),
        None => return unreadable(),
    };
    verdict.noting(&switched)
}

/// Judge the work on `branch`, checked out with nothing uncommitted in `tree`
/// at `dir`.
fn judge_branch(dir: &Path, tree: &Tree, branch: &str) -> Verdict {
    let verdict = if let Some(target) = landed_on(dir, tree, Some(branch)) {
        Verdict::Dead(format!("landed on {target}"))
    } else if !git::resolves(dir, &tree.base) {
        return Verdict::Unknown(format!("its base {} no longer exists", tree.base));
    } else {
        let base = git::landing_base(&tree.source, &tree.base, &[&tree.branch, branch]);
        match git::count(dir, &format!("{base}..HEAD")) {
            None => return Verdict::Unknown(format!("it can't be compared with {base}")),
            Some(0) => Verdict::Live("nothing committed yet".to_owned()),
            Some(ahead) => Verdict::Live(format!("{ahead} commit(s) not on {base}")),
        }
    };
    // Landed work passes burn's checks by definition; this keeps it so.
    if verdict.is_dead()
        && let Some(risk) = git::unlanded_work(dir, &tree.base)
    {
        return Verdict::Unknown(format!("burn would refuse it: {risk}"));
    }
    verdict
}

/// Where `tree`, checked out at `dir` on `branch`, has landed its work, if it
/// has: the base it lands on (see [`git::landing_base`]), or what stands in for
/// that (see [`git::stand_ins`]).
pub fn landed_on(dir: &Path, tree: &Tree, branch: Option<&str>) -> Option<String> {
    let branches: Vec<&str> = [Some(tree.branch.as_str()), branch]
        .into_iter()
        .flatten()
        .collect();
    let base = git::landing_base(&tree.source, &tree.base, &branches);
    let landed = |target: &str| git::work_landed(dir, target, "HEAD", branch);
    if landed(&base) {
        return Some(base);
    }
    git::stand_ins(dir, &base, "HEAD")
        .into_iter()
        .find(|target| landed(target))
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
/// on its base, or on what stands in for that (see [`git::stand_ins`]), or
/// there is no such branch.
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
        || git::stand_ins(&tree.source, &tree.base, &tip)
            .iter()
            .any(|stand_in| on(stand_in))
}

/// Fetch the remotes that the trees' bases are on, or that local bases track,
/// once per repo, several repos at a time, pruning deleted branches so that a
/// base that is gone looks gone. A repo that can't be fetched is reported and
/// judged as last fetched; a forest whose manifest can't be read is reported
/// when it is judged.
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
                        .filter_map(|base| {
                            git::remote_of(&repo, base).or_else(|| {
                                let upstream = git::upstream_of(&repo, base)?;
                                git::remote_of(&repo, &upstream)
                            })
                        })
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
