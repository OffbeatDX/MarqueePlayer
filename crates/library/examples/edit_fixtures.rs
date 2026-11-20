//! Fixtures for the tag editor (`MP_BENCH=editing`, `tests/edit.rs`): the metadata viewer's
//! set plus every other container the scanner takes, each with awkward tags, made by
//! ffmpeg and tagged by lofty, all inside the folder given, which gets the marker file
//! `.mp-edit-fixtures` (the editor's write guard lets the bench and the tests write only
//! under a folder that has it). The folder must not exist yet, or be one this tool made
//! (it is emptied first). Never point it at a real music folder.
//!
//!     cargo run --release -p library --example edit_fixtures -- %TEMP%\mp-edit\music

#[path = "../tests/support/edit_fixtures.rs"]
mod fixtures;

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("usage: edit_fixtures <empty folder>"));
    let temp = std::env::temp_dir();
    let temp = std::fs::canonicalize(&temp).unwrap_or(temp);
    // The nearest folder of it that exists, as Windows names it.
    let parent = dir.ancestors().skip(1).find(|p| p.exists()).and_then(|p| std::fs::canonicalize(p).ok());
    assert!(parent.is_some_and(|p| p.starts_with(&temp)), "{} is not in the temporary folder ({}): refused", dir.display(), temp.display());
    fixtures::make(&dir);
    println!("fixtures in {}", dir.display());
}
