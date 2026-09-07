# AGENTS.md

Welcome to the **pg_lens** agent ecosystem. This document serves as the single source of truth for all autonomous and pair-programming AI agents (Antigravity, Claude Code, Cursor, Codex, OpenHands) and human contributors working on this repository.

---

## 1. Project Overview & Architecture

`pg_lens` is a blazing-fast, modern TUI and web dashboard for live PostgreSQL observability (rebuilding and modernizing `pg_activity` in Rust).

* **License:** **Functional Source License, Version 1.1, MIT Future License (`FSL-1.1-MIT`)** with Contributor License Agreement ([`CLA.md`](CLA.md)).
* **Crates:**
  * **`pg_lens_core`** — Headless domain layer: database polling, typed models (`DbSnapshot`), SQL queries, catalog parsing, ring buffer history, recording (`.jsonl`), and services configuration.
  * **`pg_lens_tui`** — Ratatui terminal UI using The Elm Architecture (`app.rs` model, `update()`, pure synchronous `ui/` rendering).
  * **`pg_lens_web`** — Axum HTTP server streaming snapshots via SSE, embedding the TypeScript/Vite dashboard (`frontend/`).

---

## 2. Specialized Agent Roles

Detailed prompt definitions live in [`.claude/agents/`](.claude/agents/):

| Agent | Scope & Role | Key Responsibilities |
| :--- | :--- | :--- |
| **`release-manager`** | Shipping & Publishing | Workspace version bump, changelog, keybindings audit, regenerating `docs/demo.gif` with VHS, updating `site/index.html`, crates.io publish, and Homebrew tap verification. |
| **`lens-builder`** | Implementation | Builds new lenses, panels, and data sources end-to-end (SQL file → `queries.rs` → `db.rs` parser → core model → `poller.rs` → TUI view → web dashboard → mock data → unit tests). |
| **`qa-tester`** | Verification & Hardening | Runs static gate, PTY e2e harnesses (`e2e_pty.py`), CLI parsing matrix, and live-DB Docker testing matrix across Postgres 13–17. |
| **`feature-discovery`** | Product Discovery (Read-Only) | Researches PostgreSQL catalogs, compares competitive tools, and proposes high-value observability features with effort/risk estimates. |

---

## 3. Mandatory Quality Gates

Every code-modifying agent MUST ensure the following checks pass with **exit code 0** before declaring work complete:

```sh
# 1. Static linter (zero warnings tolerated)
cargo clippy --workspace --all-targets -- -D warnings

# 2. Complete workspace test suite
cargo test --workspace

# 3. Third-party license compliance (Fair Source / permissive check)
cargo-deny check licenses

# 4. PTY Terminal E2E Harness (verifies real VT output & screen stability)
python3 scripts/e2e_pty.py

# 5. Frontend tests & typecheck (when web code is touched)
cd crates/pg_lens_web/frontend && npm ci && npm test && npm run build
```

---

## 4. Hard Architectural Invariants

* **No UI in Core:** `pg_lens_core` MUST NEVER import or reference `ratatui`, `crossterm`, or any UI types. It must always compile cleanly for headless consumers.
* **Synchronous UI Views:** No `.await` anywhere under `crates/pg_lens_tui/src/ui/`. The view function is 100% pure and synchronous.
* **No Panics in Core:** No `.unwrap()` in `pg_lens_core/src/`. All database errors become `PollerStatus::Error`, never panics.
* **Pooler-Safe Transactions:** Every query in `poller.rs` MUST execute inside a per-tick read-only transaction (`begin_read`), never on a bare client connection.
* **Version-Gated SQL:** New SQL queries must be version-gated with the `post_NNNNNN.sql` convention; minimum supported PostgreSQL is 13.
* **Deterministic Licensing:** `python3 scripts/generate_licenses.py` must match `THIRD_PARTY_LICENSES.md` without Git diffs.

---

## 5. Visual Assets & Documentation Policy

* **Demo GIF:** The README and project landing page rely on `docs/demo.gif`. Whenever TUI layout, tabs, or major keybindings change, regenerate it via:
  ```sh
  cargo build --release -p pg_lens_tui
  vhs docs/demo.tape
  ```
* **Landing Page (`site/`):** The site at `site/index.html` must always reflect the current number of lenses (e.g. 8 lenses), accurate keybindings, active CLI subcommands (`replay`, `licenses`, `serve`), direct Changelog links in header and footer nav, hero version pill, and active license (`FSL-1.1-MIT`). Test building with:
  ```sh
  node site/build.mjs
  ```

