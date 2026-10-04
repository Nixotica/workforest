//! Sparse trees: a tree that checks out only some directories of its repo,
//! leaving the main checkout and other trees whole.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use super::Sandbox;

impl Sandbox {
    /// `repo` with `top.txt`, `src/a/x`, `src/b/y` and `docs/z` on `main`,
    /// pushed so that trees planted off `origin/main` have them.
    fn big_repo(&self, repo: &str) -> PathBuf {
        let dir = self.repo(repo);
        for file in ["top.txt", "src/a/x", "src/b/y", "docs/z"] {
            let path = dir.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, file).unwrap();
        }
        self.git(&dir, &["add", "."]);
        self.git(&dir, &["commit", "--quiet", "--message", "layout"]);
        self.git(&dir, &["push", "--quiet", "origin", "main"]);
        dir
    }
}

/// The files checked out at `dir`, relative to it, sorted.
fn files(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(next) = stack.pop() {
        for entry in fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().unwrap() == ".git" {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else {
                found.push(path.strip_prefix(dir).unwrap().display().to_string());
            }
        }
    }
    found.sort();
    found
}

#[test]
fn a_sparse_tree_checks_out_only_its_directories() {
    let sb = Sandbox::new();
    let api = sb.big_repo("api");
    let everything = ["README", "docs/z", "src/a/x", "src/b/y", "top.txt"];

    let out = sb.ok(
        &sb.root,
        &["new", "thin", "repos/api", "--sparse", "src/a", "docs"],
    );
    assert!(
        out.contains("(branch thin, off origin/main, only src/a docs)"),
        "{out}"
    );
    sb.ok(&sb.root, &["new", "whole", "repos/api"]);

    let thin = sb.forest("thin").join("api");
    assert_eq!(files(&thin), ["README", "docs/z", "src/a/x", "top.txt"]);
    assert_eq!(files(&sb.forest("whole").join("api")), everything);
    assert_eq!(files(&api), everything);
    assert_eq!(sb.git(&thin, &["status", "--porcelain"]), "");
    // The main checkout keeps no sparse checkout of its own.
    let main_sparse = Command::new("git")
        .args([
            "-C",
            api.to_str().unwrap(),
            "config",
            "--get",
            "core.sparseCheckout",
        ])
        .output()
        .unwrap();
    assert!(!main_sparse.status.success(), "{main_sparse:?}");

    let status = sb.ok(&sb.root, &["status", "thin"]);
    assert!(status.contains(", sparse: "), "{status}");
    assert!(
        status.contains("src/a") && status.contains("docs"),
        "{status}"
    );
    assert!(!sb.ok(&sb.root, &["status", "whole"]).contains("sparse"));
    let json: Value =
        serde_json::from_str(&sb.ok(&sb.root, &["status", "thin", "--json"])).unwrap();
    let mut dirs: Vec<&str> = json["trees"][0]["sparse"]
        .as_array()
        .unwrap()
        .iter()
        .map(|dir| dir.as_str().unwrap())
        .collect();
    dirs.sort();
    assert_eq!(dirs, ["docs", "src/a"]);

    // A sparse tree burns like any other.
    sb.ok(&sb.root, &["burn", "thin"]);
    assert!(!sb.forest("thin").exists());
}

#[test]
fn plant_makes_a_sparse_tree_of_an_existing_branch() {
    let sb = Sandbox::new();
    let api = sb.big_repo("api");
    sb.git(&api, &["branch", "topic"]);
    sb.ok(&sb.root, &["new", "later"]);

    sb.ok(
        &sb.root,
        &[
            "plant",
            "-f",
            "later",
            "repos/api",
            "--branch",
            "topic",
            "--sparse",
            "src/b",
        ],
    );

    let tree = sb.forest("later").join("api");
    assert_eq!(sb.git(&tree, &["branch", "--show-current"]), "topic");
    assert_eq!(files(&tree), ["README", "src/b/y", "top.txt"]);
}

#[test]
fn sparse_directories_must_be_inside_the_repo() {
    let sb = Sandbox::new();
    let api = sb.big_repo("api");

    for dir in ["../outside", "/abs", "-x", "", "./src"] {
        let sparse = format!("--sparse={dir}");
        let err = sb.fails(&sb.root, &["new", "bad", "repos/api", &sparse]);
        assert!(
            err.contains("--sparse takes directories inside the repo"),
            "{dir:?}: {err}"
        );
        assert!(!sb.forest("bad").exists(), "{dir:?}");
    }
    assert_eq!(sb.git(&api, &["branch", "--list", "bad"]), "");
}
