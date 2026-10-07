//! Settings: where forests live, what their branches are called, and how
//! build caches are grafted into them.
//!
//! Each setting comes from its environment variable, else the config file at
//! `$XDG_CONFIG_HOME/workforest/config.toml`, else its default.

use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use toml_edit::{Array, DocumentMut, Item, value};

use crate::error::{Context, Result, bail};

pub struct Config {
    /// Directory holding one subdirectory per forest.
    pub forest_root: PathBuf,
    forest_root_source: Source,
    file: ConfigFile,
}

/// Where a setting's value comes from.
#[derive(Clone, Debug)]
pub enum Source {
    Env(&'static str),
    File(PathBuf),
    Default,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Env(var) => write!(f, "${var}"),
            Source::File(path) => write!(f, "{}", path.display()),
            Source::Default => f.write_str("default"),
        }
    }
}

/// A setting's value, and where it came from.
pub struct Setting<T> {
    pub value: T,
    pub source: Source,
}

/// One setting as `workforest config` shows it: its value, or why the value
/// is unusable, and where it comes from.
pub struct Shown {
    pub name: &'static str,
    pub value: std::result::Result<String, String>,
    pub source: Source,
}

impl Config {
    /// Read the config file, if there is one, and work out where forests live.
    /// A config file that isn't valid TOML stops every command.
    pub fn load() -> Result<Config> {
        let file = ConfigFile::read()?;
        let root = forest_root(&file)?;
        Ok(Config {
            forest_root: root.value,
            forest_root_source: root.source,
            file,
        })
    }

    /// The size in bytes from which a grafted cache file is hardlinked rather
    /// than copied, 64 KiB by default. Only commands that graft read it, so a
    /// bad value can't stop the others.
    pub fn link_min(&self) -> Result<Setting<u64>> {
        const VAR: &str = "WORKFOREST_CACHE_LINK_MIN";
        if let Some(value) = env_value(VAR) {
            return match value.parse() {
                Ok(bytes) => Ok(Setting {
                    value: bytes,
                    source: Source::Env(VAR),
                }),
                Err(_) => bail!("{VAR} must be a number of bytes, not '{value}'"),
            };
        }
        if let Some(item) = self.file.get(&["cache", "link_min"]) {
            return match item
                .as_integer()
                .and_then(|bytes| u64::try_from(bytes).ok())
            {
                Some(bytes) => Ok(Setting {
                    value: bytes,
                    source: self.file.source(),
                }),
                None => bail!(
                    "cache.link_min in {} must be a number of bytes",
                    self.file.path.display()
                ),
            };
        }
        Ok(Setting {
            value: 64 * 1024,
            source: Source::Default,
        })
    }

    /// The prefix of the branch a tree gets when none is given, such as
    /// `nix/` to plant forest `login` on `nix/login`; none by default. Only
    /// commands that plant read it, so a bad value can't stop the others.
    pub fn branch_prefix(&self) -> Result<Setting<String>> {
        const VAR: &str = "WORKFOREST_BRANCH_PREFIX";
        if let Some(prefix) = env_value(VAR) {
            return Ok(Setting {
                value: prefix,
                source: Source::Env(VAR),
            });
        }
        if let Some(item) = self.file.get(&["branch_prefix"]) {
            return match item.as_str() {
                Some(prefix) => Ok(Setting {
                    value: prefix.to_owned(),
                    source: self.file.source(),
                }),
                None => bail!(
                    "branch_prefix in {} must be a string",
                    self.file.path.display()
                ),
            };
        }
        Ok(Setting {
            value: String::new(),
            source: Source::Default,
        })
    }

    /// The directories where repos named on the command line are looked for:
    /// `$WORKFOREST_REPOS`, colon-separated like `PATH`, else `repos` in the
    /// config file, one path or a list, which `workforest setup` writes. There
    /// is no default: without either, repos must be given as paths.
    pub fn repos(&self) -> Result<Setting<Vec<PathBuf>>> {
        const VAR: &str = "WORKFOREST_REPOS";
        if let Some(dirs) = env::var_os(VAR).filter(|dirs| !dirs.is_empty()) {
            return Ok(Setting {
                value: env::split_paths(&dirs)
                    .filter(|dir| !dir.as_os_str().is_empty())
                    .collect(),
                source: Source::Env(VAR),
            });
        }
        let Some(item) = self.file.get(&["repos"]) else {
            return Ok(Setting {
                value: Vec::new(),
                source: Source::Default,
            });
        };
        let dirs = match item.as_array() {
            Some(list) => list
                .iter()
                .map(|dir| match dir.as_str() {
                    Some(dir) => self.file.expand("repos", dir),
                    None => bail!("repos in {} must be paths", self.file.path.display()),
                })
                .collect::<Result<_>>()?,
            None => vec![self.file.path_value("repos", item)?],
        };
        Ok(Setting {
            value: dirs,
            source: self.file.source(),
        })
    }

    /// Write `dirs` as `repos` in the config file, creating it if need be and
    /// keeping everything else in it as it was. Directories under the home
    /// directory are written as `~/...`.
    pub fn write_repos(&self, dirs: &[PathBuf]) -> Result<()> {
        let shown: Vec<String> = dirs.iter().map(|dir| tilde(dir)).collect();
        let mut doc = self.file.doc.clone().unwrap_or_default();
        doc["repos"] = match shown.as_slice() {
            [one] => value(one.as_str()),
            many => value(many.iter().map(String::as_str).collect::<Array>()),
        };
        let path = &self.file.path;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).context(format!("could not create {}", dir.display()))?;
        }
        let staged = path.with_extension("toml.tmp");
        fs::write(&staged, doc.to_string())
            .context(format!("could not write {}", staged.display()))?;
        fs::rename(&staged, path).context(format!("could not replace {}", path.display()))
    }

    /// The config file's path, and whether it exists.
    pub fn file(&self) -> (&Path, bool) {
        (&self.file.path, self.file.doc.is_some())
    }

    /// Every setting, as `workforest config` shows it.
    pub fn shown(&self) -> Vec<Shown> {
        let link_min = self.link_min();
        let repos = self.repos();
        let branch_prefix = self.branch_prefix();
        vec![
            Shown {
                name: "forest_root",
                value: Ok(self.forest_root.display().to_string()),
                source: self.forest_root_source.clone(),
            },
            Shown {
                name: "repos",
                source: match &repos {
                    Ok(setting) => setting.source.clone(),
                    Err(_) => self.file.source(),
                },
                value: repos
                    .map(|setting| match setting.value.as_slice() {
                        [] => "not set: run `workforest setup`".to_owned(),
                        dirs => env::join_paths(dirs)
                            .map(|dirs| dirs.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                    })
                    .map_err(|err| err.to_string()),
            },
            Shown {
                name: "branch_prefix",
                source: match &branch_prefix {
                    Ok(setting) => setting.source.clone(),
                    Err(_) => self.file.source(),
                },
                value: branch_prefix
                    .map(|setting| match setting.value.as_str() {
                        "" => "none".to_owned(),
                        prefix => prefix.to_owned(),
                    })
                    .map_err(|err| err.to_string()),
            },
            Shown {
                name: "cache.link_min",
                source: match &link_min {
                    Ok(setting) => setting.source.clone(),
                    Err(_) if env_value("WORKFOREST_CACHE_LINK_MIN").is_some() => {
                        Source::Env("WORKFOREST_CACHE_LINK_MIN")
                    }
                    Err(_) => self.file.source(),
                },
                value: link_min
                    .map(|setting| setting.value.to_string())
                    .map_err(|err| err.to_string()),
            },
        ]
    }

    /// Settings in the config file that workforest doesn't know, such as a
    /// misspelt one, which is otherwise ignored.
    pub fn unknown_settings(&self) -> Vec<String> {
        const KNOWN: [&str; 4] = ["forest_root", "repos", "branch_prefix", "cache.link_min"];
        let Some(doc) = &self.file.doc else {
            return Vec::new();
        };
        let mut unknown = Vec::new();
        for (key, item) in doc.iter() {
            match item.as_table_like() {
                Some(table) if key == "cache" => {
                    for (inner, _) in table.iter() {
                        let name = format!("{key}.{inner}");
                        if !KNOWN.contains(&name.as_str()) {
                            unknown.push(name);
                        }
                    }
                }
                _ if KNOWN.contains(&key) => {}
                _ => unknown.push(key.to_owned()),
            }
        }
        unknown
    }
}

/// `$WORKFOREST_ROOT`, else `forest_root` in the config file, else
/// `~/.workforest`.
fn forest_root(file: &ConfigFile) -> Result<Setting<PathBuf>> {
    const VAR: &str = "WORKFOREST_ROOT";
    if let Some(dir) = env::var_os(VAR).filter(|dir| !dir.is_empty()) {
        return Ok(Setting {
            value: PathBuf::from(dir),
            source: Source::Env(VAR),
        });
    }
    if let Some(item) = file.get(&["forest_root"]) {
        return Ok(Setting {
            value: file.path_value("forest_root", item)?,
            source: file.source(),
        });
    }
    Ok(Setting {
        value: home()?.join(".workforest"),
        source: Source::Default,
    })
}

/// The config file, parsed: absent unless it exists.
struct ConfigFile {
    path: PathBuf,
    doc: Option<DocumentMut>,
}

impl ConfigFile {
    /// `$XDG_CONFIG_HOME/workforest/config.toml`, with `XDG_CONFIG_HOME`
    /// defaulting to `~/.config`, on macOS too.
    fn path() -> Result<PathBuf> {
        let config_home = match env::var_os("XDG_CONFIG_HOME") {
            Some(dir) if Path::new(&dir).is_absolute() => PathBuf::from(dir),
            _ => home()?.join(".config"),
        };
        Ok(config_home.join("workforest").join("config.toml"))
    }

    fn read() -> Result<ConfigFile> {
        let path = ConfigFile::path()?;
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok(ConfigFile { path, doc: None });
            }
            Err(err) => bail!("could not read {}: {err}", path.display()),
        };
        match text.parse::<DocumentMut>() {
            Ok(doc) => Ok(ConfigFile {
                path,
                doc: Some(doc),
            }),
            Err(err) => bail!("{} is not valid TOML: {err}", path.display()),
        }
    }

    fn source(&self) -> Source {
        Source::File(self.path.clone())
    }

    /// The item at `keys`, a table path such as `["cache", "link_min"]`.
    fn get(&self, keys: &[&str]) -> Option<&Item> {
        let (first, rest) = keys.split_first()?;
        let mut item = self.doc.as_ref()?.get(first)?;
        for key in rest {
            item = item.get(key)?;
        }
        Some(item)
    }

    /// A path setting: absolute, or starting with `~/` for the home directory.
    fn path_value(&self, name: &str, item: &Item) -> Result<PathBuf> {
        let Some(text) = item.as_str() else {
            bail!("{name} in {} must be a path", self.path.display());
        };
        self.expand(name, text)
    }

    /// `text`, a path that is absolute or starts with `~/`, with `~` expanded.
    fn expand(&self, name: &str, text: &str) -> Result<PathBuf> {
        let path = expand_home(text)?;
        if !path.is_absolute() {
            bail!(
                "{name} in {} must be absolute or start with ~/, not '{text}'",
                self.path.display()
            );
        }
        Ok(path)
    }
}

/// `text` with a leading `~` or `~/` standing for the home directory.
pub fn expand_home(text: &str) -> Result<PathBuf> {
    Ok(match text.strip_prefix('~') {
        Some("") => home()?,
        Some(rest) if rest.starts_with('/') => home()?.join(rest.trim_start_matches('/')),
        _ => PathBuf::from(text),
    })
}

/// `path`, with the home directory shown as `~`.
pub fn tilde(path: &Path) -> String {
    let Ok(home) = home() else {
        return path.display().to_string();
    };
    match path.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// An environment variable's value, unless it is unset or empty.
fn env_value(var: &str) -> Option<String> {
    env::var_os(var)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string_lossy().into_owned())
}

/// `$HOME`, which must be set.
pub fn home() -> Result<PathBuf> {
    match env::var_os("HOME") {
        Some(home) if !home.is_empty() => Ok(PathBuf::from(home)),
        _ => bail!("HOME is not set"),
    }
}
