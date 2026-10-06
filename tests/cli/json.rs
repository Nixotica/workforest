//! `--json`: the shape of what `ls`, `status`, `cache status` and `fire` print
//! for scripts and agents.

use std::fs;

use serde_json::Value;

use super::{Merge, Sandbox, path};

impl Sandbox {
    /// Run workforest expecting success and JSON on stdout, and parse it.
    fn json(&self, args: &[&str]) -> Value {
        let out = self.ok(&self.root, args);
        let value: Value = serde_json::from_str(&out).unwrap_or_else(|err| panic!("{err}: {out}"));
        assert_eq!(value["schema"], 1, "{value:#}");
        value
    }
}

#[test]
fn ls_json_lists_forests_and_their_trees() {
    let sb = Sandbox::new();
    let api = sb.repo("api");
    sb.ok(&sb.root, &["new", "one", "repos/api"]);
    sb.ok(&sb.root, &["new", "empty"]);

    let all = sb.json(&["ls", "--json"]);
    assert_eq!(all["forest_root"], path(&sb.forests()));
    let names: Vec<&str> = all["forests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|forest| forest["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["empty", "one"]);

    let one = sb.json(&["ls", "one", "--json"]);
    let tree = &one["forests"][0]["trees"][0];
    assert_eq!(one["forests"].as_array().unwrap().len(), 1);
    assert_eq!(tree["repo"], "api");
    assert_eq!(tree["path"], path(&sb.forest("one").join("api")));
    assert_eq!(tree["source"], path(&api));
    assert_eq!(tree["branch"], "one");
    assert_eq!(tree["base"], "origin/main");
}

#[test]
fn status_json_reports_each_tree() {
    let sb = Sandbox::new();
    for repo in ["api", "web", "docs", "gone"] {
        sb.repo(repo);
    }
    sb.ok(
        &sb.root,
        &[
            "new",
            "st",
            "repos/api",
            "repos/web",
            "repos/docs",
            "repos/gone",
        ],
    );
    let forest = sb.forest("st");
    sb.commit(&forest.join("api"), "unpushed.txt");
    fs::write(forest.join("web").join("scratch"), "x").unwrap();
    fs::remove_dir_all(forest.join("gone")).unwrap();

    let status = sb.json(&["status", "st", "--json"]);
    assert_eq!(status["name"], "st");
    let tree = |repo: &str| {
        status["trees"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tree| tree["repo"] == repo)
            .unwrap()
            .clone()
    };
    let api = tree("api");
    assert_eq!(
        (
            &api["state"],
            &api["ahead"],
            &api["behind"],
            &api["unpushed"]
        ),
        (&"clean".into(), &1.into(), &0.into(), &1.into())
    );
    assert_eq!(tree("web")["state"], "dirty");
    let docs = tree("docs");
    assert_eq!(
        (&docs["state"], &docs["unpushed"]),
        (&"clean".into(), &Value::Null)
    );
    let gone = tree("gone");
    assert_eq!(
        (&gone["state"], &gone["ahead"]),
        (&"missing".into(), &Value::Null)
    );
}

#[test]
fn cache_status_json_counts_what_each_cache_shares() {
    let sb = Sandbox::new();
    let api = sb.repo("api");
    fs::write(api.join(".gitignore"), "build/\n").unwrap();
    fs::write(api.join(".workforest-cache"), "build clone -\nlogs never\n").unwrap();
    sb.git(&api, &["add", ".gitignore", ".workforest-cache"]);
    sb.git(&api, &["commit", "--quiet", "--message", "caches"]);
    sb.git(&api, &["push", "--quiet", "origin", "main"]);
    fs::create_dir(api.join("build")).unwrap();
    fs::write(api.join("build/big"), vec![7u8; 70_000]).unwrap();
    fs::write(api.join("build/small"), "x").unwrap();
    sb.ok(&sb.root, &["new", "warm", "repos/api"]);

    let report = sb.json(&["cache", "status", "warm", "--json"]);

    let tree = &report["trees"][0];
    assert_eq!(tree["missing"], false);
    let build = &tree["caches"][0];
    assert_eq!(build["path"], "build");
    assert_eq!(build["mode"], "clone");
    assert_eq!(build["state"], "grafted");
    assert_eq!(build["files"], 2);
    assert_eq!(build["shared_bytes"], 70_000);
    assert_eq!(build["own_bytes"], 1);
    // Grafted copies keep their times, so the tree's newest file is as old as
    // the main checkout's.
    assert!(
        build["newest"].as_u64().is_some_and(|secs| secs > 0),
        "{build}"
    );
    assert_eq!(build["newest"], build["main_newest"]);
    assert_eq!(tree["caches"][1]["state"], "never");
}

#[test]
fn fire_json_reports_every_forest_and_what_was_done() {
    let sb = Sandbox::new();
    sb.repo("api");
    sb.pull_request("landed", "api");
    sb.merge_on_remote("api", "landed", Merge::Squash);
    sb.ok(&sb.root, &["new", "fresh", "repos/api"]);

    let plan = sb.json(&["fire", "--json"]);
    assert_eq!(plan["dry_run"], true);
    let forests = plan["forests"].as_array().unwrap();
    assert_eq!(forests.len(), 2);
    let fresh = &forests[0];
    assert_eq!(
        (&fresh["name"], &fresh["action"]),
        (&"fresh".into(), &"keep".into())
    );
    assert_eq!(fresh["trees"][0]["verdict"], "live");
    assert_eq!(fresh["trees"][0]["reason"], "nothing committed yet");
    let landed = &forests[1];
    assert_eq!(
        (&landed["action"], &landed["done"]),
        (&"burn".into(), &Value::Null)
    );
    assert_eq!(landed["trees"][0]["verdict"], "dead");

    // Burning still prints only JSON.
    let burned = sb.json(&["fire", "--json", "--yes"]);
    assert_eq!(burned["dry_run"], false);
    assert_eq!(burned["forests"][1]["done"], true);
    assert_eq!(burned["forests"][1]["error"], Value::Null);
    assert!(!sb.forest("landed").exists());
}
