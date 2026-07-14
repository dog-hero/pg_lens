# pg_lens 🔬🐘

> **A blazing-fast, modern TUI for PostgreSQL observability.**
> *A microscopic view into your PostgreSQL performance.*

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
![Rust](https://img.shields.io/badge/Rust-2024_edition-orange?logo=rust)
![PostgreSQL](https://img.shields.io/badge/PostgreSQL-13%2B-336791?logo=postgresql&logoColor=white)

`pg_lens` connects to a running PostgreSQL server (13+) and renders live
activity in your terminal — inspired by [pg_activity](https://github.com/dalibo/pg_activity)
and [btop](https://github.com/aristocratos/btop), built in Rust with
[ratatui](https://ratatui.rs) for minimal overhead: a **~4 MiB static
binary** that idles at **~7 MB of RSS** while monitoring a loaded server.

<!-- TODO: record a demo gif with vhs (https://github.com/charmbracelet/vhs)
     and embed it here. No screenshot yet — do not fake one. -->

## Features

- **Macro Lens** — instance vitals at a glance: connection saturation gauge,
  TPS with scrolling sparkline history, cache hit ratio, active sessions,
  deadlocks and temp-file counters, server uptime.
- **Micro Lens** — per-backend activity table: state, wait events, running
  duration, and a status marker for **blocked** (`B`, red) and **waiting**
  (`W`, yellow) sessions, powered by `pg_locks` + `pg_blocking_pids()`.
- **Query detail panel** — press `Enter` on any row to read the full SQL.
- **Resilient by design** — if the database goes down, pg_lens shows an
  error banner, keeps the last known data on screen, and reconnects with
  backoff. The UI never blocks on the database.
- **Live tuning** — adjust the poll interval on the fly with `+` / `-`
  (0.5s–10s).
- **Version-aware queries** — dedicated query sets for PostgreSQL 13, 14+,
  and 16+, following pg_activity's versioning convention.
- **Single static binary** — no runtime, no dependencies; musl builds run
  on any Linux, including Alpine containers.

## Installation

### From source

Requires Rust (edition 2024, tested with cargo 1.93):

```sh
git clone git@github.com:dog-hero/pg_lens.git
cd pg_lens
cargo build --release -p pg_lens_tui
./target/release/pg_lens_tui --mock   # instant demo, no database needed
```

### Static Linux binaries (musl)

No host toolchain setup needed — build inside Docker:

```sh
# aarch64 (arm64 hosts)
docker run --rm -v "$PWD":/work -w /work -e CARGO_TARGET_DIR=/work/target-musl \
  rust:1-alpine sh -c 'apk add -q musl-dev && cargo build --release -p pg_lens_tui'

# x86_64 (from an arm64 host, via emulation)
docker run --rm --platform linux/amd64 -v "$PWD":/work -w /work \
  -e CARGO_TARGET_DIR=/work/target-musl-amd64 \
  rust:1-alpine sh -c 'apk add -q musl-dev && cargo build --release -p pg_lens_tui'
```

With rustup available (e.g. in CI), the leaner recipe is
`rustup target add {x86_64,aarch64}-unknown-linux-musl` +
[`cargo-zigbuild`](https://github.com/rust-cross/cargo-zigbuild) or
[`cross`](https://github.com/cross-rs/cross).

## Usage

```sh
pg_lens_tui --dsn "host=localhost port=5432 user=postgres password=..." [--interval 2]
pg_lens_tui --mock          # built-in mock data (dev/demo mode)
```

| Flag / env | Meaning |
|---|---|
| `--dsn <DSN>` | Connection string: `key=value` DSN or `postgres://` URL. Also read from the `PG_LENS_DSN` env var. Default: `host=localhost user=postgres` |
| `--interval <secs>` | Poll interval in seconds (minimum 0.5). Default: 2 |
| `--mock` | Use built-in mock data instead of a real database |

> **Tip:** for production monitoring, use a read-only role granted the
> [`pg_monitor`](https://www.postgresql.org/docs/current/predefined-roles.html)
> predefined role in the DSN.

### Keybindings

| Key | Action |
|---|---|
| `q` / `Esc` | Quit (`Esc` first closes the detail view if open) |
| `Tab` | Switch between Macro Lens and Micro Lens |
| `j` / `k` / `↓` / `↑` | Move selection in the activity table |
| `s` | Cycle sort column (duration / state / pid) |
| `Enter` | Open/close query detail for the selected row |
| `+` / `-` | Increase / decrease the poll interval |

## Architecture

Cargo workspace with a strict frontend-agnostic core:

```
              ┌──────────────────────────────────────┐
              │ pg_lens_core                         │
              │  poller ──► watch<Arc<DbSnapshot>>   │
              │  (versioned SQL, ring-buffer history)│
              └───────────────┬──────────────────────┘
                              │  last-value-wins, N consumers
                    ┌─────────┴──────────┐
                    ▼                    ▼
              pg_lens_tui          pg_lens_web (roadmap)
              ratatui / TEA        axum + SSE + TypeScript
```

- **`pg_lens_core`** — UI-free domain layer: serializable models, versioned
  SQL queries (adapted from pg_activity), `tokio-postgres` data access, the
  poller task, and a fixed-capacity ring buffer for metric history.
- **`pg_lens_tui`** — [The Elm Architecture](https://guide.elm-lang.org/architecture/)
  (model → update → view): a single `mpsc<Action>` channel aggregates
  keyboard input and snapshot updates; the render path is 100% synchronous
  and never awaits.

The poller publishes `Arc<DbSnapshot>` through a `tokio::sync::watch`
channel so any number of frontends can consume the same data — that is how
the upcoming web dashboard plugs in without touching the core.

## Development

```sh
cargo test --workspace                                   # unit + integration tests
cargo clippy --workspace --all-targets -- -D warnings    # lint gate (must be clean)
python3 scripts/e2e_pty.py                               # end-to-end TUI test on a real PTY (mock data)
python3 scripts/e2e_pty_live.py                          # end-to-end against a live PostgreSQL
```

To test against real PostgreSQL versions locally:

```sh
docker run -d --name pglens_pg16 -e POSTGRES_PASSWORD=pg -p 54316:5432 postgres:16
cargo run -p pg_lens_tui -- --dsn "host=localhost port=54316 user=postgres password=pg"
```

See [CLAUDE.md](CLAUDE.md) for architecture invariants and
[PLAN.md](PLAN.md) for the full development plan.

## Roadmap

- [ ] **Web Lens** — `pg_lens serve`: an axum-based web dashboard consuming
      the same watch channel (SSE streaming, TypeScript + uPlot frontend
      embedded in the binary, token auth, read-only).
- [ ] Admin actions (`pg_cancel_backend` / `pg_terminate_backend`)
- [ ] `pg_stat_statements` integration
- [ ] Replication / WAL sender views
- [ ] Prometheus export
- [ ] Demo gif

## Contributing

Issues and pull requests are welcome. Before submitting:

1. `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` must pass.
2. Keep `pg_lens_core` free of any UI dependency — it must compile for
   headless consumers.
3. New SQL must be version-gated (see `pg_lens_core/queries/`); minimum
   supported PostgreSQL is 13.

## License

[MIT](LICENSE)

## Acknowledgements

- [pg_activity](https://github.com/dalibo/pg_activity) (Dalibo) — the
  original inspiration; pg_lens adapts its battle-tested system queries.
- [ratatui](https://ratatui.rs) — the TUI framework powering the interface.
