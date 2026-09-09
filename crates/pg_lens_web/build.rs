//! Fails the build early — with actionable instructions — when the compiled
//! frontend (`frontend/dist/`) is missing.
//!
//! `dist/` is a build artifact and is NOT committed: building with the
//! `web` feature requires running the frontend build first (CI does this
//! automatically). Without this check, rust-embed would embed an empty
//! tree or emit a cryptic derive error.

use std::path::Path;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let dist_index = Path::new(&manifest_dir).join("frontend/dist/index.html");
    if !dist_index.exists() {
        panic!(
            "\n\npg_lens_web: `frontend/dist/index.html` not found.\n\
             The web UI bundle is missing. Build it with:\n\n    \
             cd crates/pg_lens_web/frontend && npm ci && npm run build\n\n\
             (Or build the TUI alone: cargo build -p pg_lens_tui \
             --no-default-features)\n"
        );
    }
    // Re-embed when the bundle changes; re-run this check if dist vanishes.
    println!("cargo:rerun-if-changed=frontend/dist");
    println!("cargo:rerun-if-changed=frontend/src");
    println!("cargo:rerun-if-changed=frontend/index.html");

    // Warn if source files are newer than the compiled dist bundle.
    if let Ok(dist_meta) = dist_index.metadata() {
        if let Ok(dist_time) = dist_meta.modified() {
            let src_index = Path::new(&manifest_dir).join("frontend/index.html");
            if let Ok(src_meta) = src_index.metadata() {
                if let Ok(src_time) = src_meta.modified() {
                    if src_time > dist_time {
                        println!(
                            "cargo:warning=pg_lens_web: frontend source is newer than frontend/dist/. \
                             Run 'cd crates/pg_lens_web/frontend && npm run build' to bundle the latest web assets."
                        );
                    }
                }
            }
        }
    }
}
