//! The globs that pick out cache files to copy whatever their size.
//!
//! A glob containing `/` matches a file's path inside the cache, at any depth:
//! `.fingerprint/*` matches `debug/.fingerprint/foo/lib-foo`. Any other glob
//! matches the file's name. `*` matches any run of characters, `/` included, and
//! `?` matches one character, as in `find -path`.

use std::path::Path;

/// A glob, parsed once so that matching it against every file in a cache
/// allocates nothing.
pub struct Glob {
    pattern: Vec<char>,
    /// For a glob containing `/`, the pattern behind `*/`, which matches it
    /// at any depth.
    anywhere: Option<Vec<char>>,
}

impl Glob {
    pub fn new(glob: &str) -> Glob {
        Glob {
            pattern: glob.chars().collect(),
            anywhere: glob
                .contains('/')
                .then(|| format!("*/{glob}").chars().collect()),
        }
    }

    /// Whether the file at `rel`, a path inside a cache, matches.
    pub fn matches(&self, rel: &Path) -> bool {
        match &self.anywhere {
            Some(anywhere) => {
                let rel = rel.to_string_lossy();
                wildcard(&self.pattern, &rel) || wildcard(anywhere, &rel)
            }
            None => rel
                .file_name()
                .is_some_and(|name| wildcard(&self.pattern, &name.to_string_lossy())),
        }
    }
}

/// Whether all of `text` matches `pattern`, where `*` matches any run of
/// characters and `?` matches one.
fn wildcard(pattern: &[char], text: &str) -> bool {
    // `p` indexes the pattern's characters, `t` the text's bytes.
    let (mut p, mut t) = (0, 0);
    // Where the last `*` was, and where in the text it started matching.
    let mut star: Option<(usize, usize)> = None;
    while let Some(c) = text[t..].chars().next() {
        match pattern.get(p) {
            Some('*') => {
                star = Some((p, t));
                p += 1;
            }
            Some(&want) if want == '?' || want == c => {
                p += 1;
                t += c.len_utf8();
            }
            _ => match star {
                // Let the last `*` swallow one more character and retry.
                Some((star_p, star_t)) => {
                    let swallowed = text[star_t..].chars().next().map_or(1, char::len_utf8);
                    star = Some((star_p, star_t + swallowed));
                    p = star_p + 1;
                    t = star_t + swallowed;
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches_path(glob: &str, rel: &str) -> bool {
        Glob::new(glob).matches(Path::new(rel))
    }

    fn wild(pattern: &str, text: &str) -> bool {
        wildcard(&pattern.chars().collect::<Vec<_>>(), text)
    }

    #[test]
    fn name_globs_match_the_file_name_at_any_depth() {
        assert!(matches_path(".cargo-lock", ".cargo-lock"));
        assert!(matches_path(".cargo-lock", "debug/.cargo-lock"));
        assert!(matches_path("*.d", "debug/deps/foo-0123.d"));
        assert!(!matches_path("*.d", "debug/deps/foo.d.bak"));
        assert!(!matches_path("*.d", "debug/foo.d/bin"));
        assert!(matches_path("lib?.a", "out/libx.a"));
        assert!(!matches_path("lib?.a", "out/libxy.a"));
    }

    #[test]
    fn path_globs_match_a_trailing_run_of_components() {
        assert!(matches_path(".fingerprint/*", ".fingerprint/foo/lib-foo"));
        assert!(matches_path(
            ".fingerprint/*",
            "debug/.fingerprint/foo-0123/lib-foo"
        ));
        assert!(!matches_path(".fingerprint/*", "debug/fingerprint/foo"));
        assert!(!matches_path(".fingerprint/*", "debug/x.fingerprint/foo"));
        assert!(matches_path("build/*/output", "debug/build/foo-1/output"));
        assert!(!matches_path("build/*/output", "debug/build/foo-1/stderr"));
    }

    #[test]
    fn stars_backtrack() {
        assert!(wild("a*b*c", "a-b-b-c"));
        assert!(wild("*", ""));
        assert!(wild("**", "abc"));
        assert!(!wild("a*b", "a-c"));
        assert!(!wild("", "a"));
    }

    #[test]
    fn question_marks_and_stars_match_characters_not_bytes() {
        assert!(wild("?.d", "é.d"));
        assert!(wild("*é*", "café-x"));
        assert!(!wild("??.d", "é.d"));
    }
}
