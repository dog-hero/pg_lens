//! Fails the build early — with actionable instructions — when the compiled
//! frontend (`frontend/dist/`) is missing.
//!
//! The built `dist/` is committed to the repository precisely so that plain
//! `cargo build` / `cargo install` works without a Node toolchain. This
//! check exists for the case where someone deletes `dist/` (or a fresh
//! `npm run build` failed half-way): rust-embed would otherwise embed an
//! empty tree or emit a cryptic derive error.

use std::path::Path;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let dist_index = Path::new(&manifest_dir).join("frontend/dist/index.html");
    if !dist_index.exists() {
        panic!(
            "\n\npg_lens_web: `frontend/dist/index.html` not found.\n\
             The web UI bundle is missing. Build it with:\n\n    \
             cd crates/pg_lens_web/frontend && npm ci && npm run build\n\n\
             (The built dist/ is normally committed, so this only happens \
             after deleting it or on a broken build.)\n"
        );
    }
    // Re-embed when the bundle changes; re-run this check if dist vanishes.
    println!("cargo:rerun-if-changed=frontend/dist");
}
