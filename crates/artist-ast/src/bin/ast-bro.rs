//! Test fixture only — artist drives `artist-ast` as a library.
//!
//! The vendored integration suite in `tests/` is end-to-end and execs
//! `CARGO_BIN_EXE_ast-bro`, so this target exists to keep that coverage
//! runnable. Built only with the `cli` feature.

fn main() {
    artist_ast::cli::run();
}
