//! Python's virtual environments, which a copy leaves out.

use std::path::Path;

use super::left_out::LeftOut;

/// A venv: its `pyvenv.cfg` and its scripts name the path it was made at, so a
/// copy's `pip` would install into the main checkout's.
pub const VENV: LeftOut = LeftOut {
    recognizes: is_venv,
    why: "a Python venv names the path it was made at",
};

fn is_venv(dir: &Path) -> bool {
    dir.join("pyvenv.cfg").is_file()
}
