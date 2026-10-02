//! Forests on disk: where they live, what they may be called, and the
//! `.workforest` manifest recording where each tree came from.
//!
//! The manifest is one tab-separated line per tree: repo, source repo, branch,
//! base ref.

use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;

use crate::config::Config;
use crate::error::{Context, Result, bail};

const MANIFEST: &str = ".workforest";

/// A forest: a directory under the forest root holding one tree per repo.
pub struct Forest {
    pub name: String,
    pub dir: PathBuf,
}

/// One tree in a forest, as the manifest records it.
#[derive(Clone, Debug)]
pub struct Tree {
    /// The tree's directory name in the forest, which is its repo's name.
    pub repo: String,
    /// The main worktree of the repo the tree was grafted from.
    pub source: PathBuf,
    pub branch: String,
    /// The ref the branch was created from, which status and the safety checks
    /// compare against.
    pub base: String,
}

impl Tree {
    fn parse(line: &str) -> Tree {
        let mut fields = line.splitn(4, '\t');
        let mut field = || fields.next().unwrap_or_default().to_owned();
        Tree {
            repo: field(),
            source: PathBuf::from(field()),
            branch: field(),
            base: field(),
        }
    }

    fn line(&self) -> String {
        format!(
            "{}\t{}\t{}\t{}\n",
            self.repo,
            self.source.display(),
            self.branch,
            self.base
        )
    }
}

impl Forest {
    /// Create a new forest with no trees.
    pub fn plant(config: &Config, name: &str) -> Result<Forest> {
        validate_name(name)?;
        let dir = config.forest_root.join(name);
        if dir.symlink_metadata().is_ok() {
            bail!("forest already exists: {}", dir.display());
        }
        fs::create_dir_all(&dir).context(format!("could not create {}", dir.display()))?;
        let forest = Forest {
            name: name.to_owned(),
            dir,
        };
        forest.write_trees(Vec::new())?;
        Ok(forest)
    }

    /// The existing forest called `name`.
    pub fn named(config: &Config, name: &str) -> Result<Forest> {
        validate_name(name)?;
        let dir = config.forest_root.join(name);
        if !dir.is_dir() {
            bail!("no such forest: {name}");
        }
        Ok(Forest {
            name: name.to_owned(),
            dir,
        })
    }

    /// The forest called `name`, or else the one containing the current
    /// directory. `hint` tells the user how to name a forest instead.
    pub fn resolve(config: &Config, name: Option<&str>, hint: &str) -> Result<Forest> {
        match name.map(str::to_owned).or_else(|| containing_cwd(config)) {
            Some(name) => Forest::named(config, &name),
            None => bail!("not inside a forest; {hint}"),
        }
    }

    /// Every forest under the forest root, sorted by name.
    pub fn all(config: &Config) -> Result<Vec<Forest>> {
        let root = &config.forest_root;
        let entries = match fs::read_dir(root) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => bail!("could not read {}: {err}", root.display()),
        };
        let mut forests = Vec::new();
        for entry in entries {
            let entry = entry.context(format!("could not read {}", root.display()))?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if name.starts_with('.') || !entry.path().is_dir() {
                continue;
            }
            forests.push(Forest {
                name,
                dir: entry.path(),
            });
        }
        forests.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(forests)
    }

    pub fn tree_dir(&self, repo: &str) -> PathBuf {
        self.dir.join(repo)
    }

    /// Whether the current directory is this forest's directory or inside it.
    pub fn contains_cwd(&self) -> bool {
        let (Ok(dir), Ok(cwd)) = (
            fs::canonicalize(&self.dir),
            env::current_dir().and_then(fs::canonicalize),
        ) else {
            return false;
        };
        cwd.starts_with(dir)
    }

    /// The trees the manifest records; a forest without a manifest has none.
    pub fn trees(&self) -> Result<Vec<Tree>> {
        let path = self.dir.join(MANIFEST);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => bail!("could not read {}: {err}", path.display()),
        };
        Ok(text
            .lines()
            .filter(|line| !line.is_empty())
            .map(Tree::parse)
            .collect())
    }

    /// The tree recorded for `repo`, if any.
    pub fn tree(&self, repo: &str) -> Result<Option<Tree>> {
        Ok(self.trees()?.into_iter().find(|tree| tree.repo == repo))
    }

    /// Record `tree`, replacing any earlier row for the same repo.
    pub fn record(&self, tree: Tree) -> Result<()> {
        let mut trees = self.trees()?;
        trees.retain(|existing| existing.repo != tree.repo);
        trees.push(tree);
        self.write_trees(trees)
    }

    /// Drop the row for `repo`.
    pub fn forget(&self, repo: &str) -> Result<()> {
        let mut trees = self.trees()?;
        trees.retain(|tree| tree.repo != repo);
        self.write_trees(trees)
    }

    fn write_trees(&self, trees: Vec<Tree>) -> Result<()> {
        let mut lines: Vec<String> = trees.iter().map(Tree::line).collect();
        lines.sort();
        let path = self.dir.join(MANIFEST);
        let staged = self.dir.join(format!("{MANIFEST}.tmp"));
        fs::write(&staged, lines.concat())
            .context(format!("could not write {}", staged.display()))?;
        fs::rename(&staged, &path).context(format!("could not replace {}", path.display()))
    }
}

/// A forest name must be a single, visible path component.
fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name.starts_with('.') || name.contains(['/', '\t', '\n']) {
        bail!("invalid forest name: '{name}'");
    }
    Ok(())
}

/// The name of the forest whose directory holds the current directory.
fn containing_cwd(config: &Config) -> Option<String> {
    let root = fs::canonicalize(&config.forest_root).ok()?;
    let cwd = env::current_dir().and_then(fs::canonicalize).ok()?;
    let name = cwd.strip_prefix(&root).ok()?.components().next()?;
    name.as_os_str().to_str().map(str::to_owned)
}
