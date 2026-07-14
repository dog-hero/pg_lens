# pg_lens

**A blazing-fast, modern TUI for PostgreSQL observability.**
*A microscopic view into your PostgreSQL performance.*

`pg_lens` connects to a running PostgreSQL server (13+) and renders live
activity in your terminal: a **Macro Lens** (connections, TPS with sparkline
history, cache hit ratio, longest query/transaction) and a **Micro Lens**
(per-backend activity table with wait events, blocked-query markers, and
running durations).

<!-- TODO: record a demo gif with vhs (https://github.com/charmbracelet/vhs)
     and embed it here. No screenshot yet — do not fake one. -->

## Install / Build

Requires Rust (edition 2024, tested with cargo 1.93):

```sh
git clone <repo-url> pg_lens
cd pg_lens
cargo build --release -p pg_lens_tui
./target/release/pg_lens_tui --mock   # instant demo, no database needed
```

Static Linux binaries (musl) can be built without any host toolchain setup,
using Docker:

```sh
# aarch64 (arm64 hosts)
docker run --rm -v "$PWD":/work -w /work -e CARGO_TARGET_DIR=/work/target-musl \
  rust:1-alpine sh -c 'apk add -q musl-dev && cargo build --release -p pg_lens_tui'

# x86_64 (from an arm64 host, via emulation)
docker run --rm --platform linux/amd64 -v "$PWD":/work -w /work \
  -e CARGO_TARGET_DIR=/work/target-musl-amd64 \
  rust:1-alpine sh -c 'apk add -q musl-dev && cargo build --release -p pg_lens_tui'
```

CI TODO: with rustup available, the leaner recipe is
`rustup target add {x86_64,aarch64}-unknown-linux-musl` +
[`cargo-zigbuild`](https://github.com/rust-cross/cargo-zigbuild) (or
[`cross`](https://github.com/cross-rs/cross)).

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

For production monitoring, prefer a read-only role with the `pg_monitor`
predefined role in the DSN.

## Keybindings

| Key | Action |
|---|---|
| `q` / `Esc` | Quit (`Esc` first closes the detail view if open) |
| `Tab` | Switch between Macro Lens and Micro Lens |
| `j` / `k` / `↓` / `↑` | Move selection in the activity table |
| `s` | Cycle sort column |
| `Enter` | Open/close query detail for the selected row |
| `+` / `-` | Increase / decrease the poll interval |

## Architecture

Cargo workspace with two crates:

- **`pg_lens_core`** — UI-free domain layer: models, versioned SQL queries
  (dedicated query sets for PostgreSQL 13, 14+, and 16+), `tokio-postgres`
  data access (with the mandatory `Connection` spawned on its own task), a
  poller task, and a fixed-capacity ring buffer for metric history.
- **`pg_lens_tui`** — `ratatui` + `crossterm` front-end following The Elm
  Architecture (model → update → view). The poller publishes
  `Arc<DbSnapshot>` through a `tokio::sync::watch` channel; the UI is a pure
  consumer and never awaits inside the render path.

This separation exists so future consumers (e.g. a web dashboard) can reuse
`pg_lens_core` unchanged.

## Roadmap

- **Fase 6 — Web Lens**: `pg_lens serve` — an axum-based web dashboard
  consuming the same watch channel (same data, browser-rendered, SSE
  streaming). Post-MVP.
