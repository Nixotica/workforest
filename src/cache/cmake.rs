//! CMake's build directories, which a copy leaves out.

use std::path::Path;

use super::left_out::LeftOut;

/// A configured build directory: `CMakeCache.txt` and the build files it
/// generated name the source and build directories, so building in a copy
/// would build the main checkout's sources into its build directory.
pub const BUILD: LeftOut = LeftOut {
    recognizes: is_build,
    why: "a CMake build directory names the source it was configured for",
};

fn is_build(dir: &Path) -> bool {
    dir.join("CMakeCache.txt").is_file()
}
