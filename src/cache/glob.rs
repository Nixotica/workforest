//! The globs that pick out cache files to copy whatever their size.
//!
//! A glob containing `/` matches a file's path inside the cache, at any depth:
//! `.fingerprint/*` matches `debug/.fingerprint/foo/lib-foo`. Any other glob
//! matches the file's name. `*` matches any run of characters, `/` included, and
//! `?` matches one character, as in `find -path`.

use std::path::Path;

/// Whether the file at `rel`, a path inside a cache, matches `glob`.
pub fn matches(glob: &str, rel: &Path) -> bool {
    if glob.contains('/') {
        let rel = rel.to_string_lossy();
        wildcard(glob, &rel) || wildcard(&format!("*/{glob}"), &rel)
    } else {
        rel.file_name()
            .is_some_and(|name| wildcard(glob, &name.to_string_lossy()))
    }
}

/// Whether all of `text` matches `pattern`, where `*` matches any run of
/// characters and `?` matches one.
fn wildcard(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut p, mut t) = (0, 0);
    // Where the last `*` was, and where in the text it started matching.
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        match pattern.get(p) {
            Some('*') => {
                star = Some((p, t));
                p += 1;
            }
            Some(&c) if c == '?' || c == text[t] => {
                p += 1;
                t += 1;
            }
            _ => match star {
                // Let the last `*` swallow one more character and retry.
                Some((star_p, star_t)) => {
                    star = Some((star_p, star_t + 1));
                    p = star_p + 1;
                    t = star_t + 1;
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
        matches(glob, Path::new(rel))
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
        assert!(wildcard("a*b*c", "a-b-b-c"));
        assert!(wildcard("*", ""));
        assert!(wildcard("**", "abc"));
        assert!(!wildcard("a*b", "a-c"));
        assert!(!wildcard("", "a"));
    }
}
