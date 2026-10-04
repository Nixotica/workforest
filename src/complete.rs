//! Shell completion: the scripts that register it, the names it offers, and
//! `wfcd`, a shell function that cds into a forest or one of its trees.
//!
//! Completion is dynamic: the registered script asks `workforest` itself for
//! candidates as you type, so forest, tree and repo names are always current.

use std::ffi::OsStr;
use std::io::{self, Write};

use clap_complete::engine::{CompletionCandidate, PathCompleter, ValueCompleter};
use clap_complete::env::{Bash, EnvCompleter, Fish, Zsh};

use crate::cli::{CompletionsArgs, NamesArgs, Shell};
use crate::config::Config;
use crate::error::{Context, Result};
use crate::forest::Forest;
use crate::repos;

/// Every forest's name.
pub fn forests(current: &OsStr) -> Vec<CompletionCandidate> {
    let names = Config::load()
        .and_then(|config| Forest::all(&config))
        .unwrap_or_default()
        .into_iter()
        .map(|forest| forest.name);
    matching(current, names)
}

/// The trees of the forest containing the current directory, the forest that
/// commands taking tree names act on unless `-f` says otherwise.
pub fn trees(current: &OsStr) -> Vec<CompletionCandidate> {
    let names = Config::load()
        .and_then(|config| Forest::resolve(&config, None, ""))
        .and_then(|forest| forest.trees())
        .unwrap_or_default()
        .into_iter()
        .map(|tree| tree.repo);
    matching(current, names)
}

/// Directories, and the names of the repos in the repos directories.
pub fn repos(current: &OsStr) -> Vec<CompletionCandidate> {
    let mut candidates = PathCompleter::dir().complete(current);
    if let Ok(dirs) = Config::load().and_then(|config| config.repos()) {
        candidates.extend(matching(current, repos::names(&dirs.value).into_iter()));
    }
    candidates
}

fn matching(current: &OsStr, names: impl Iterator<Item = String>) -> Vec<CompletionCandidate> {
    let current = current.to_string_lossy();
    names
        .filter(|name| name.starts_with(&*current))
        .map(CompletionCandidate::new)
        .collect()
}

/// Print the script that registers completion of `workforest` and `wf`, or of
/// the one `--bin` names.
pub fn print_registration(args: &CompletionsArgs) -> Result<()> {
    let shell: &dyn EnvCompleter = match args.shell {
        Shell::Bash => &Bash,
        Shell::Zsh => &Zsh,
        Shell::Fish => &Fish,
    };
    let bins = match &args.bin {
        Some(bin) => vec![bin.as_str()],
        None => vec!["workforest", "wf"],
    };
    let mut out = io::stdout().lock();
    for bin in bins {
        shell
            .write_registration("COMPLETE", bin, bin, "workforest", &mut out)
            .context("could not print the completion script")?;
    }
    Ok(())
}

/// Print the `wfcd` function for `shell`, with completion of its arguments.
pub fn print_shell_init(shell: Shell) -> Result<()> {
    let script = match shell {
        Shell::Bash => BASH_INIT,
        Shell::Zsh => ZSH_INIT,
        Shell::Fish => FISH_INIT,
    };
    io::stdout()
        .lock()
        .write_all(script.as_bytes())
        .context("could not print the shell functions")
}

/// Print the name of every forest, or of every tree in `forest`, one a line.
pub fn print_names(args: &NamesArgs) -> Result<()> {
    let config = Config::load()?;
    let names: Vec<String> = match &args.forest {
        Some(name) => Forest::named(&config, name)?
            .trees()?
            .into_iter()
            .map(|tree| tree.repo)
            .collect(),
        None => Forest::all(&config)?
            .into_iter()
            .map(|forest| forest.name)
            .collect(),
    };
    for name in names {
        println!("{name}");
    }
    Ok(())
}

const BASH_INIT: &str = r#"# wfcd [forest [tree]]: cd into a forest, or one of its trees. With no forest,
# the forest containing the current directory.
wfcd() {
    local dir
    dir=$(workforest path ${1:+"$1"}) || return
    if [ -n "${2-}" ]; then
        dir="$dir/$2"
    fi
    cd -- "$dir"
}

_wfcd() {
    local cur=${COMP_WORDS[COMP_CWORD]}
    local IFS=$'\n'
    case $COMP_CWORD in
        1) COMPREPLY=($(compgen -W "$(workforest __names 2>/dev/null)" -- "$cur")) ;;
        2) COMPREPLY=($(compgen -W "$(workforest __names "${COMP_WORDS[1]}" 2>/dev/null)" -- "$cur")) ;;
        *) COMPREPLY=() ;;
    esac
}
complete -F _wfcd wfcd
"#;

const ZSH_INIT: &str = r#"# wfcd [forest [tree]]: cd into a forest, or one of its trees. With no forest,
# the forest containing the current directory.
wfcd() {
    local dir
    dir=$(workforest path ${1:+"$1"}) || return
    if [[ -n $2 ]]; then
        dir="$dir/$2"
    fi
    cd -- "$dir"
}

_wfcd() {
    case $CURRENT in
        2) compadd -- ${(f)"$(workforest __names 2>/dev/null)"} ;;
        3) compadd -- ${(f)"$(workforest __names $words[2] 2>/dev/null)"} ;;
    esac
}
if (( $+functions[compdef] )); then
    compdef _wfcd wfcd
fi
"#;

const FISH_INIT: &str = r#"# wfcd [forest [tree]]: cd into a forest, or one of its trees. With no forest,
# the forest containing the current directory.
function wfcd --description 'cd into a forest, or one of its trees'
    set -l dir (workforest path $argv[1]); or return
    if set -q argv[2]
        set dir $dir/$argv[2]
    end
    cd $dir
end

complete --command wfcd --no-files --condition 'test (count (commandline --current-process --tokenize --cut-at-cursor)) -eq 1' --arguments '(workforest __names 2>/dev/null)'
complete --command wfcd --no-files --condition 'test (count (commandline --current-process --tokenize --cut-at-cursor)) -eq 2' --arguments '(workforest __names (commandline --current-process --tokenize --cut-at-cursor)[2] 2>/dev/null)'
"#;
