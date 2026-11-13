//! Gradle's in-project state, which a copy leaves out. A Gradle build doesn't
//! need it to start warm in a tree: with `org.gradle.caching`, tasks are served
//! from the build cache in Gradle's user home, which every checkout shares.

use std::path::Path;

use super::left_out::LeftOut;

/// The files whose presence makes a directory a Gradle project.
const SCRIPTS: &[&str] = &[
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
];

/// `.gradle`, the task history, file hashes and checksums of the builds run
/// in the project.
pub const PROJECT_CACHE: LeftOut = LeftOut {
    recognizes: is_project_cache,
    why: "Gradle's .gradle doesn't work at another path",
};

/// A project's `build` directory, which Gradle trusts only along with the
/// task history in `.gradle`: without it, every task reruns and overwrites
/// what a copy brought.
pub const BUILD: LeftOut = LeftOut {
    recognizes: is_build,
    why: "Gradle can't reuse it without the task history in .gradle",
};

fn is_project_cache(dir: &Path) -> bool {
    dir.file_name().is_some_and(|name| name == ".gradle")
}

fn is_build(dir: &Path) -> bool {
    dir.file_name().is_some_and(|name| name == "build")
        && dir
            .parent()
            .is_some_and(|project| SCRIPTS.iter().any(|script| project.join(script).is_file()))
}
