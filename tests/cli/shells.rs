//! Shell completion and `wfcd`, in each shell that is installed. A shell that
//! isn't is skipped; `nix flake check` installs them all.

use std::path::Path;
use std::process::Command;

use super::Sandbox;

impl Sandbox {
    /// Run `script` in `shell` from `cwd`, with the built workforest first on
    /// the PATH, and return its stdout, or `None` if `shell` isn't installed.
    fn shell(&self, shell: &str, cwd: &Path, script: &str) -> Option<String> {
        let bin = Path::new(env!("CARGO_BIN_EXE_workforest"))
            .parent()
            .unwrap();
        let path = format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut command = Command::new(shell);
        self.isolate(&mut command)
            .env("PATH", path)
            .env("WORKFOREST_REPOS", self.repos())
            .current_dir(cwd)
            .args(["-c", script]);
        let output = match command.output() {
            Ok(output) => output,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("{shell} isn't installed; skipped");
                return None;
            }
            Err(err) => panic!("could not run {shell}: {err}"),
        };
        assert!(
            output.status.success(),
            "{shell} failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Some(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Two forests: `alpha` with trees of `api` and `web`, and an empty `beta`.
    fn two_forests(&self) {
        self.repo("api");
        self.repo("web");
        self.ok(&self.root, &["new", "alpha", "repos/api", "repos/web"]);
        self.ok(&self.root, &["new", "beta"]);
    }
}

fn lines(out: &str) -> Vec<&str> {
    // fish follows each candidate with a tab and its description.
    out.lines()
        .map(|line| line.split('\t').next().unwrap())
        .collect()
}

#[test]
fn bash_completes_forests_trees_and_repos_for_workforest_and_wf() {
    let sb = Sandbox::new();
    sb.two_forests();
    let script = r#"
        source <(workforest completions bash)
        show() { printf '%s\n' "${COMPREPLY[@]}"; echo --; }
        COMP_WORDS=(workforest burn ''); COMP_CWORD=2
        _clap_complete_workforest workforest '' burn; show
        COMP_WORDS=(wf status al); COMP_CWORD=2
        _clap_complete_wf wf al status; show
        COMP_WORDS=(wf cut a); COMP_CWORD=2
        _clap_complete_wf wf a cut; show
        COMP_WORDS=(workforest plant w); COMP_CWORD=2
        _clap_complete_workforest workforest w plant; show
    "#;
    let Some(out) = sb.shell("bash", &sb.forest("alpha").join("api"), script) else {
        return;
    };
    let blocks: Vec<Vec<&str>> = out
        .split("--\n")
        .map(|block| block.lines().collect())
        .collect();
    assert!(
        blocks[0].contains(&"alpha") && blocks[0].contains(&"beta"),
        "{out}"
    );
    assert_eq!(blocks[1], ["alpha"], "{out}");
    assert_eq!(blocks[2], ["api"], "{out}");
    assert_eq!(blocks[3], ["web"], "{out}");
}

#[test]
fn zsh_and_fish_completion_scripts_load_and_answer() {
    let sb = Sandbox::new();
    sb.two_forests();
    // What the zsh script asks for, as zsh asks it.
    let zsh = r#"
        workforest completions zsh > completions.zsh
        zsh -n completions.zsh
        COMPLETE=zsh _CLAP_COMPLETE_INDEX=2 _CLAP_IFS=$'\n' workforest -- wf burn ''
    "#;
    if let Some(out) = sb.shell("zsh", &sb.root, zsh) {
        let names = lines(&out);
        assert!(names.contains(&"alpha") && names.contains(&"beta"), "{out}");
    }
    let fish = r#"
        workforest completions fish | source
        complete --do-complete 'workforest burn '
        echo ==
        complete --do-complete 'wf ls b'
    "#;
    if let Some(out) = sb.shell("fish", &sb.root, fish) {
        let (burn, ls) = out.split_once("==\n").unwrap();
        let burn = lines(burn);
        assert!(burn.contains(&"alpha") && burn.contains(&"beta"), "{out}");
        assert_eq!(lines(ls), ["beta"], "{out}");
    }
}

#[test]
fn wfcd_goes_to_a_forest_or_one_of_its_trees_in_every_shell() {
    let sb = Sandbox::new();
    sb.two_forests();
    let alpha = sb.forest("alpha");
    let expected = format!(
        "{}\n{}\n{}\n",
        alpha.display(),
        alpha.join("web").display(),
        alpha.display()
    );
    let posix = |shell: &str| {
        format!(
            r#"
            eval "$(workforest shell-init {shell})"
            wfcd alpha && pwd
            wfcd alpha web && pwd
            cd ../api && wfcd && pwd
            "#
        )
    };
    for shell in ["bash", "zsh"] {
        if let Some(out) = sb.shell(shell, &sb.root, &posix(shell)) {
            assert_eq!(out, expected, "{shell}");
        }
    }
    let fish = r#"
        workforest shell-init fish | source
        wfcd alpha; and pwd
        wfcd alpha web; and pwd
        cd ../api; and wfcd; and pwd
    "#;
    if let Some(out) = sb.shell("fish", &sb.root, fish) {
        assert_eq!(out, expected, "fish");
    }

    let bash = r#"
        eval "$(workforest shell-init bash)"
        COMP_WORDS=(wfcd ''); COMP_CWORD=1; _wfcd; printf '%s\n' "${COMPREPLY[@]}"
        COMP_WORDS=(wfcd alpha ''); COMP_CWORD=2; _wfcd; printf '%s\n' "${COMPREPLY[@]}"
    "#;
    if let Some(out) = sb.shell("bash", &sb.root, bash) {
        assert_eq!(out, "alpha\nbeta\napi\nweb\n");
    }
    let fish = r#"
        workforest shell-init fish | source
        complete --do-complete 'wfcd '
        complete --do-complete 'wfcd alpha '
    "#;
    if let Some(out) = sb.shell("fish", &sb.root, fish) {
        assert_eq!(lines(&out), ["alpha", "beta", "api", "web"]);
    }
}
