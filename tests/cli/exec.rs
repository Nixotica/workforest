//! `exec`: a command in every tree, its output grouped by tree, and its
//! failures counted.

use std::fs;

use super::{Sandbox, path};

impl Sandbox {
    /// A forest `both` with trees of `api` and `web`.
    fn forest_of_two(&self) {
        self.repo("api");
        self.repo("web");
        self.ok(&self.root, &["new", "both", "repos/api", "repos/web"]);
    }
}

#[test]
fn exec_runs_in_each_tree_in_order_under_headers() {
    let sb = Sandbox::new();
    sb.forest_of_two();
    let forest = sb.forest("both");

    let out = sb.ok(
        &forest,
        &[
            "exec",
            "--",
            "sh",
            "-c",
            "echo $WORKFOREST_FOREST/$WORKFOREST_TREE; pwd",
        ],
    );

    assert_eq!(
        out,
        format!(
            "=== api ===\nboth/api\n{}\n=== web ===\nboth/web\n{}\n",
            path(&forest.join("api")),
            path(&forest.join("web"))
        )
    );
    // -f names the forest from anywhere.
    let named = sb.ok(&sb.root, &["exec", "-f", "both", "--", "true"]);
    assert_eq!(named, "=== api ===\n=== web ===\n");
}

#[test]
fn exec_in_parallel_prints_each_tree_whole_as_it_finishes() {
    let sb = Sandbox::new();
    sb.forest_of_two();
    // api starts first but finishes last; each writes around a pause, so
    // their output would interleave if it weren't gathered.
    let script = r#"
        [ "$WORKFOREST_TREE" = api ] && sleep 0.6
        echo "$WORKFOREST_TREE start"
        echo "$WORKFOREST_TREE error" >&2
        sleep 0.2
        echo "$WORKFOREST_TREE end"
    "#;

    let out = sb.ok(
        &sb.forest("both"),
        &["exec", "--parallel", "--", "sh", "-c", script],
    );

    assert_eq!(
        out,
        "=== web ===\nweb start\nweb error\nweb end\n\
         === api ===\napi start\napi error\napi end\n"
    );
    let two = sb.ok(
        &sb.forest("both"),
        &["exec", "--parallel", "2", "--", "true"],
    );
    assert!(
        two.contains("=== api ===\n") && two.contains("=== web ===\n"),
        "{two}"
    );
}

#[test]
fn exec_fails_listing_the_trees_it_failed_in() {
    let sb = Sandbox::new();
    sb.forest_of_two();
    sb.repo("docs");
    sb.ok(&sb.root, &["plant", "-f", "both", "repos/docs"]);
    fs::remove_dir_all(sb.forest("both").join("docs")).unwrap();
    let fail_in_web = r#"[ "$WORKFOREST_TREE" != web ] || exit 3"#;

    for parallel in [&[][..], &["--parallel"][..]] {
        let mut args = vec!["exec"];
        args.extend(parallel);
        args.extend(["--", "sh", "-c", fail_in_web]);
        let output = sb.workforest(&sb.forest("both"), &args);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{parallel:?}: {stderr}");
        assert!(
            stderr.contains(
                "failed in 2 of 3 tree(s):\n  docs: its directory is gone\n  web: exit status 3\n"
            ) || stderr.contains(
                "failed in 2 of 3 tree(s):\n  web: exit status 3\n  docs: its directory is gone\n"
            ),
            "{parallel:?}: {stderr}"
        );
        // The trees after a failure still ran.
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("=== web ===\n"), "{stdout}");
    }
}

#[test]
fn exec_reports_a_command_that_cannot_start() {
    let sb = Sandbox::new();
    sb.forest_of_two();

    for parallel in [&[][..], &["--parallel"][..]] {
        let mut args = vec!["exec"];
        args.extend(parallel);
        args.extend(["--", "no-such-program-for-workforest"]);
        let err = sb.fails(&sb.forest("both"), &args);
        assert!(err.contains("failed in 2 of 2 tree(s):"), "{err}");
        assert!(err.contains("  api: could not start: "), "{err}");
    }
    let bare = sb.fails(&sb.forest("both"), &["exec"]);
    assert!(bare.contains("<COMMAND>"), "{bare}");
}
