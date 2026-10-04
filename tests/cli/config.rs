//! The config file: what it sets, and which setting wins.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use super::Sandbox;

impl Sandbox {
    /// The config file the sandbox's `XDG_CONFIG_HOME` points at.
    fn config_file(&self) -> PathBuf {
        self.root.join("home/.config/workforest/config.toml")
    }

    fn write_config(&self, toml: &str) {
        let path = self.config_file();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, toml).unwrap();
    }

    /// Run workforest without `WORKFOREST_ROOT`, so that the config file and
    /// the defaults decide where forests live, with `env` set besides.
    fn unrooted(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_workforest"));
        self.isolate(&mut command).env_remove("WORKFOREST_ROOT");
        command
            .envs(env.iter().copied())
            .current_dir(&self.root)
            .args(args)
            .output()
            .expect("run workforest")
    }
}

fn stdout(output: &Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    assert!(!output.status.success(), "should have failed");
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn settings_come_from_the_environment_then_the_config_file_then_defaults() {
    let sb = Sandbox::new();
    sb.repo("api");
    let home = sb.root.join("home");
    let shown = |env: &[(&str, &str)]| stdout(&sb.unrooted(&["config"], env));

    let defaults = shown(&[]);
    assert!(
        defaults.starts_with(&format!("{} (not found)\n", sb.config_file().display())),
        "{defaults}"
    );
    assert!(
        defaults.contains(&format!(
            "  {:<16} {:<32} default\n",
            "forest_root",
            home.join(".workforest").display()
        )),
        "{defaults}"
    );
    assert!(
        defaults.contains(&format!(
            "  {:<16} {:<32} default\n",
            "cache.link_min", 65536
        )),
        "{defaults}"
    );

    sb.write_config("forest_root = \"~/trees\"\n\n[cache]\nlink_min = 4096\n");
    let from_file = shown(&[]);
    let file = sb.config_file().display().to_string();
    for (name, value) in [
        ("forest_root", home.join("trees").display().to_string()),
        ("cache.link_min", "4096".to_owned()),
    ] {
        assert!(
            from_file.contains(&format!("  {name:<16} {value:<32} {file}\n")),
            "{from_file}"
        );
    }
    stdout(&sb.unrooted(&["new", "filed", "repos/api"], &[]));
    assert!(home.join("trees/filed/api").is_dir());

    let elsewhere = sb.root.join("elsewhere");
    let elsewhere = elsewhere.to_str().unwrap();
    let env = [
        ("WORKFOREST_ROOT", elsewhere),
        ("WORKFOREST_CACHE_LINK_MIN", "1"),
    ];
    let from_env = shown(&env);
    assert!(
        from_env.contains(&format!(
            "  {:<16} {elsewhere:<32} $WORKFOREST_ROOT\n",
            "forest_root"
        )),
        "{from_env}"
    );
    assert!(
        from_env.contains(&format!(
            "  {:<16} {:<32} $WORKFOREST_CACHE_LINK_MIN\n",
            "cache.link_min", 1
        )),
        "{from_env}"
    );
    stdout(&sb.unrooted(&["new", "envied", "repos/api"], &env));
    assert!(sb.root.join("elsewhere/envied/api").is_dir());
}

#[test]
fn a_bad_link_min_stops_only_the_commands_that_graft() {
    let sb = Sandbox::new();
    sb.repo("api");
    sb.write_config("[cache]\nlink_min = \"big\"\n");

    let refusal = stderr(&sb.unrooted(&["new", "graft", "repos/api"], &[]));
    assert!(
        refusal.contains(&format!(
            "cache.link_min in {} must be a number of bytes",
            sb.config_file().display()
        )),
        "{refusal}"
    );
    stdout(&sb.unrooted(&["new", "cold", "repos/api", "--no-cache"], &[]));
    stdout(&sb.unrooted(&["ls"], &[]));
    let shown = stdout(&sb.unrooted(&["config"], &[]));
    assert!(shown.contains("cache.link_min   unusable: "), "{shown}");
    let env = [("WORKFOREST_CACHE_LINK_MIN", "4096")];
    stdout(&sb.unrooted(&["new", "warm", "repos/api"], &env));
}

#[test]
fn a_config_file_that_is_not_toml_stops_every_command() {
    let sb = Sandbox::new();
    sb.write_config("forest_root = \n");

    let refusal = stderr(&sb.unrooted(&["ls"], &[]));

    assert!(
        refusal.contains(&format!("{} is not valid TOML", sb.config_file().display())),
        "{refusal}"
    );
}

#[test]
fn forest_root_in_the_config_file_must_be_absolute_or_under_home() {
    let sb = Sandbox::new();
    sb.write_config("forest_root = \"trees\"\n");
    let refusal = stderr(&sb.unrooted(&["ls"], &[]));
    assert!(
        refusal.contains("forest_root in ")
            && refusal.contains("must be absolute or start with ~/, not 'trees'"),
        "{refusal}"
    );

    sb.write_config("forest_root = 7\n");
    let refusal = stderr(&sb.unrooted(&["ls"], &[]));
    assert!(refusal.contains("must be a path"), "{refusal}");

    let absolute = sb.root.join("abs");
    sb.write_config(&format!("forest_root = '{}'\n", absolute.display()));
    stdout(&sb.unrooted(&["new", "there"], &[]));
    assert!(absolute.join("there").is_dir());
}

#[test]
fn config_warns_about_settings_it_does_not_know() {
    let sb = Sandbox::new();
    sb.write_config("forest-root = \"~/x\"\n\n[cache]\nlinkmin = 1\nlink_min = 2\n");

    let output = sb.unrooted(&["config"], &[]);

    let warnings = String::from_utf8_lossy(&output.stderr);
    let file = sb.config_file().display().to_string();
    assert!(output.status.success(), "{warnings}");
    assert!(
        warnings.contains(&format!(
            "workforest: {file}: unknown setting forest-root\n"
        )),
        "{warnings}"
    );
    assert!(
        warnings.contains(&format!(
            "workforest: {file}: unknown setting cache.linkmin\n"
        )),
        "{warnings}"
    );
    assert!(!warnings.contains("cache.link_min"), "{warnings}");
}

#[test]
fn the_config_file_follows_xdg_config_home() {
    let sb = Sandbox::new();
    let custom = sb.root.join("xdg");
    fs::create_dir_all(custom.join("workforest")).unwrap();
    fs::write(
        custom.join("workforest/config.toml"),
        "forest_root = \"~/from-xdg\"\n",
    )
    .unwrap();
    sb.write_config("forest_root = \"~/from-default\"\n");

    let custom_env = [("XDG_CONFIG_HOME", custom.to_str().unwrap())];
    let shown = stdout(&sb.unrooted(&["config"], &custom_env));
    assert!(shown.contains("from-xdg"), "{shown}");

    // Unset or relative, XDG_CONFIG_HOME means ~/.config.
    for value in ["", "relative/dir"] {
        let shown = stdout(&sb.unrooted(&["config"], &[("XDG_CONFIG_HOME", value)]));
        assert!(shown.contains("from-default"), "{value:?}: {shown}");
    }
}
