# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

pg_lens is a Rust TUI for live PostgreSQL observability (a pg_activity rebuild). `PLAN.md` is the authoritative phased development plan — phases 1–5 (MVP) are complete; phase 6 (Web Lens, an axum + SSE + TypeScript web frontend) is next. Read the relevant phase in PLAN.md before starting work: each phase carries its own verification checklist and anti-pattern list, and those checks are enforced before every commit.

## Commands

```sh
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings   # must be clean; this is the merge gate
cargo test --workspace
cargo test -p pg_lens_tui <test_name>                    # single test
cargo run -p pg_lens_tui -- --mock                       # run TUI with mock data (needs a TTY)
cargo run -p pg_lens_tui -- --dsn "host=localhost port=54316 user=postgres password=pg"
```

End-to-end verification (the TUI can't be tested by piping stdin — these allocate a real PTY and parse VT output):

```sh
python3 scripts/e2e_pty.py          # mock-mode e2e; PG_LENS_E2E_COLS/ROWS env override terminal size
python3 scripts/e2e_pty_live.py     # against a live DB; supports --expect-micro-growing, --expect-blocked-marker
```

Live-DB testing convention: `docker run -d --name pglens_pg16 -e POSTGRES_PASSWORD=pg -p 54316:5432 postgres:16` (ports 54313/54314 for PG 13/14), wait with `pg_isready`, generate load with `pgbench` inside the container, and `docker rm -f` the containers when done. If `docker pull` hangs on `docker-credential-desktop`, retry with a clean anonymous `DOCKER_CONFIG` dir.

Static Linux binaries: this machine has Homebrew rust (no rustup), so musl cross-compiles run inside `rust:1-alpine` Docker — recipe in README.md.

## Architecture

Cargo workspace, two crates (a third, `pg_lens_web`, arrives in phase 6):

- **`pg_lens_core`** — frontend-agnostic domain layer: `models.rs` (all `serde::Serialize` — the future web UI streams them as JSON), `db.rs`, `queries.rs` + `queries/*.sql`, `poller.rs`, `history.rs`.
- **`pg_lens_tui`** — ratatui frontend using The Elm Architecture: `app.rs` holds the `App` model, the `Action` enum, and `update()` (the only place state mutates); `ui/` is pure synchronous rendering; `event.rs` turns crossterm events into `Action`s.

Data flow: the poller task queries Postgres each tick and publishes `Arc<DbSnapshot>` on a `tokio::sync::watch` channel (last-value-wins, multi-consumer — the web UI will subscribe to the same channel). In the TUI, a bridge task converts watch updates into `Action::Snapshot` on the single `mpsc<Action>` that `main.rs`'s `select!` loop consumes. A second `watch<Duration>` flows the other way: `+`/`-` keys adjust the poll interval live. Poller errors never crash the UI — they travel as `PollerStatus::Error` inside the snapshot envelope (last good data retained, banner shown, reconnect with backoff).

SQL lives in `pg_lens_core/queries/*.sql` (adapted from dalibo/pg_activity), loaded via `include_str!` and selected by `server_version_num` — the `post_140000` suffix convention means "PG >= 14". Minimum supported version is PG 13; `query_id` only exists on 14+, which is why activity has two variants. `SnapshotHistory` (ring buffer, cap 120) is owned by the poller and a copy ships inside every snapshot so all frontends see the same series.

## Hard invariants (grep-audited before commits; see PLAN.md phase 5)

- `pg_lens_core` never references ratatui, crossterm, or the TUI's `Action` enum.
- No `.await` anywhere under `crates/pg_lens_tui/src/ui/` — the view is 100% synchronous.
- No `unwrap()` in `pg_lens_core/src/` — DB errors become `PollerStatus`, never panics.
- No `std::thread::sleep` / `block_on`; no shared `Mutex` between tasks — data crosses via watch/mpsc messages only.
- The tokio-postgres `Connection` must be `tokio::spawn`ed (done in `db.rs`) — queries silently hang without it.
- Every poll query runs inside a per-tick **read-only transaction** (`begin_read` in `poller.rs`), never a bare `client.query`. This keeps pg_lens working behind a connection pooler (prepare + execute stay on one backend) and lets `SET LOCAL statement_timeout` hold there; it also gives the fast tick a single consistent snapshot. Admin actions use `begin_write`. NOTE: PgBouncer *transaction* pooling still breaks tokio-postgres's named prepared statements (names leak across backends → `prepared statement already exists`); pg_lens works on direct connections, restricted roles, and *session* pooling — transaction pooling needs PgBouncer ≥1.21 `max_prepared_statements>0`. A full `simple_query` (unnamed-protocol) rewrite is the only way to lift that, deliberately deferred.

## tokio-postgres type traps (learned the hard way in phase 3)

- `EXTRACT(epoch FROM ...)` returns `numeric` on PG >= 14 but `float8` on 13 — always cast `::float8` in the SQL.
- `inet` columns (`client_addr`) have no default Rust mapping — cast `::text` in the SQL.
- Aggregate `sum(...)`/`count(*)` need explicit `::int8`/`::int4` casts for `Row::try_get`.
