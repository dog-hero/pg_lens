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

### Homebrew (macOS / Linux)

```sh
brew install dog-hero/tap/pg_lens
```

The [tap](https://github.com/dog-hero/homebrew-tap) serves the prebuilt
release binaries and its formula is updated automatically by the release
workflow. Installing via brew avoids the macOS Gatekeeper prompt entirely
(no quarantine attribute is applied).

### Prebuilt binaries (releases)

Download the archive for your platform from the
[releases page](https://github.com/dog-hero/pg_lens/releases). On macOS,
prefer `curl` — browser downloads get the quarantine attribute and
Gatekeeper will refuse to run the unsigned binary:

```sh
# macOS (Apple Silicon)
curl -L https://github.com/dog-hero/pg_lens/releases/download/v0.2.1/pg_lens-v0.2.1-aarch64-apple-darwin.tar.gz | tar xz
./pg_lens-v0.2.1-aarch64-apple-darwin/pg_lens --mock
```

If you already downloaded it with a browser and macOS says the app
"cannot be verified", clear the quarantine flag once:

```sh
xattr -d com.apple.quarantine ./pg_lens
```

(The binaries are not yet signed/notarized with an Apple Developer ID —
building from source or installing via curl avoids the prompt entirely.)

### Docker (GHCR)

Multi-arch images (linux/amd64 + linux/arm64) are published to
[GHCR](https://github.com/dog-hero/pg_lens/pkgs/container/pg_lens) on
every release. The default command serves the [Web Lens](#web-lens)
dashboard on `0.0.0.0:8080`:

```sh
docker run --rm -p 8080:8080 \
  -e PG_LENS_AUTH_TOKEN="$(openssl rand -hex 32)" \
  -e PGHOST=db.internal -e PGUSER=monitor -e PGPASSWORD=secret \
  ghcr.io/dog-hero/pg_lens
```

**`PG_LENS_AUTH_TOKEN` is required for the default command**: pg_lens
refuses to bind a non-loopback address without a token (a container has
to listen beyond loopback to be reachable, so the image inherits that
gate). Without the env var the container exits immediately with
`refusing to listen on non-loopback address 0.0.0.0:8080 without
authentication`. With it, every `/api` route requires
`Authorization: Bearer <token>`.

Connection settings come from the standard libpq env vars (`PGHOST`,
`PGPORT`, `PGUSER`, `PGPASSWORD`, ...) or any `pg_lens` flag appended to
the run command. A minimal compose file:

```yaml
services:
  db:
    image: postgres:16
    environment:
      POSTGRES_PASSWORD: pg
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U postgres"]
      interval: 5s
      timeout: 3s
      retries: 10
  pg_lens:
    image: ghcr.io/dog-hero/pg_lens:latest
    ports: ["8080:8080"]
    environment:
      PG_LENS_AUTH_TOKEN: change-me
      PGHOST: db
      PGUSER: postgres
      PGPASSWORD: pg
    depends_on:
      db:
        condition: service_healthy
```

The entrypoint is the `pg_lens` binary itself, so any arguments replace
the default `serve` command — the TUI works too:

```sh
docker run -it --rm ghcr.io/dog-hero/pg_lens tui \
  --dsn "host=db.internal user=monitor password=secret"
```

The image runs as `nobody` (uid 65534) on Alpine. `sh` is present, so
the [services file](#services-file)'s `password_cmd` works — mount the
file readable by uid 65534 and pass
`--services-file /path/to/services.toml --service <name>`.

### deb / rpm (Linux servers)

Every release attaches `.deb` and `.rpm` packages (amd64 + arm64), built
with [nfpm](https://nfpm.goreleaser.com) from the same static musl
binaries as the tarballs. The package is named `pg-lens` (Debian policy
forbids `_` in package names); it installs `/usr/bin/pg_lens` plus docs
and has no dependencies.

```sh
# Debian / Ubuntu (pick amd64 or arm64)
curl -LO https://github.com/dog-hero/pg_lens/releases/download/v0.2.1/pg-lens_0.2.1_amd64.deb
sudo dpkg -i pg-lens_0.2.1_amd64.deb

# RHEL / Fedora / SUSE (x86_64 or aarch64)
curl -LO https://github.com/dog-hero/pg_lens/releases/download/v0.2.1/pg-lens-0.2.1-1.x86_64.rpm
sudo rpm -i pg-lens-0.2.1-1.x86_64.rpm    # or: sudo dnf install ./pg-lens-0.2.1-1.x86_64.rpm
```

### From source

Requires Rust (edition 2024, tested with cargo 1.93):

```sh
git clone git@github.com:dog-hero/pg_lens.git
cd pg_lens
cargo build --release -p pg_lens_tui
./target/release/pg_lens --mock   # instant demo, no database needed
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
pg_lens --dsn "host=localhost port=5432 user=postgres password=..." [--interval 2]
pg_lens --mock          # built-in mock data (dev/demo mode)
```

| Flag / env | Meaning |
|---|---|
| `--dsn <DSN>` | Connection string: `key=value` DSN or `postgres://` URL. Also read from the `PG_LENS_DSN` env var. Optional — see [Connecting](#connecting) |
| `--service <name>` | Connect using a named entry from the [services file](#services-file). Also read from `PG_LENS_SERVICE`, falling back to `PGSERVICE`. Mutually exclusive with `--dsn` |
| `--services-file <path>` | Services file location. Default: `$XDG_CONFIG_HOME/pg_lens/services.toml` (or `~/.config/pg_lens/services.toml`). Also read from `PG_LENS_SERVICES_FILE` |
| `--list-services` | Print the defined services (names + host/user, never secrets) and exit |
| `--interval <secs>` | Poll interval in seconds (minimum 0.5). Default: 2 |
| `--mock` | Use built-in mock data instead of a real database |

> **Tip:** for production monitoring, use a read-only role granted the
> [`pg_monitor`](https://www.postgresql.org/docs/current/predefined-roles.html)
> predefined role in the DSN.

### Connecting

`pg_lens` resolves the connection the way `psql` does: any field the
`--dsn` sets wins; anything it leaves out falls back to the standard
[libpq environment variables](https://www.postgresql.org/docs/current/libpq-envars.html);
whatever is still missing gets the defaults `host=localhost user=postgres`.

```sh
# no --dsn at all — pure environment:
PGHOST=db.internal PGPORT=5432 PGUSER=pg_monitor_ro PGPASSWORD=... pg_lens

# mix and match — the DSN pins the host, the env supplies the password:
PGPASSWORD=... pg_lens --dsn "host=db.internal user=pg_monitor_ro"
```

| Env var | Maps to | Notes |
|---|---|---|
| `PGHOST` | `host` | hostname or Unix-socket directory |
| `PGPORT` | `port` | must be a valid TCP port |
| `PGDATABASE` | `dbname` | |
| `PGUSER` | `user` | default: `postgres` |
| `PGPASSWORD` | `password` | never displayed or logged |
| `PGAPPNAME` | `application_name` | |
| `PGCONNECT_TIMEOUT` | connect timeout | whole seconds; `0` = wait indefinitely |

**Precedence (highest first):** `--dsn` field → [services-file](#services-file)
entry → env var → default (`host=localhost`, `user=postgres`) — the same
order libpq uses. Empty env values count as unset. The header shows the
resolved `user@host` — the password never appears anywhere.

### Services file

For more than one database, register named services in
`~/.config/pg_lens/services.toml` (inspired by libpq's `pg_service.conf`,
with one extra trick: `password_cmd` runs an external command and uses its
stdout as the password, so the file never has to contain a secret):

```toml
[services.prod]
host = "db.prod.internal"
port = 5432
user = "pg_monitor_ro"
dbname = "app"
application_name = "pg_lens"
connect_timeout_secs = 5
password_cmd = "vault kv get -field=password secret/pg/prod"

[services.staging]
host = "db.staging.internal"
user = "postgres"
# sugar: a password of the form "$(...)" is treated as password_cmd
password = "$(op read op://infra/pg-staging/password)"

[services.local]
host = "localhost"
user = "postgres"
# macOS Keychain works too:
password_cmd = "security find-generic-password -s pg_local -w"
```

```sh
pg_lens --service prod       # or: PG_LENS_SERVICE=prod / PGSERVICE=prod
pg_lens --list-services      # names + host/user, never secrets
```

Any field a service leaves out falls through to the env vars and defaults
above; `--dsn` fields always win (and the `--dsn`/`--service` flags are
mutually exclusive). `password_cmd` runs as `sh -c <cmd>` with a 10s
timeout, and is **re-executed on every (re)connection attempt** — so
short-lived tokens (Vault leases, SSO helpers) keep working across
reconnects. If the command fails, the TUI stays alive and shows the error
(stderr, never stdout) in the banner, retrying with backoff.

> **Security note:** this file can execute commands — treat it like code and
> keep it at `0600`. `pg_lens` refuses a services file that is writable by
> group/others, and refuses one that combines a plaintext `password` with
> group/other read permission. A plaintext `password` works but is
> discouraged; prefer `password_cmd`.

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

## Web Lens

`pg_lens serve` hosts the same Macro/Micro Lens as a live web dashboard —
vitals cards, a TPS/active-sessions chart (uPlot), and a sortable activity
table with blocked/waiting row highlighting — streamed over Server-Sent
Events from the same poller the TUI uses. Read-only by design: no
cancel/terminate actions are exposed over HTTP.

<!-- TODO: screenshot of the web dashboard -->

### Quickstart

```sh
pg_lens serve --mock                      # demo data, http://127.0.0.1:8080
pg_lens serve --dsn "host=... user=..."   # against a real server
pg_lens serve --listen 127.0.0.1:9000     # different port (default 8080)
```

Open the printed address in a browser — the dashboard updates in real time
on every poll.

The frontend (Vite + TypeScript, `crates/pg_lens_web/frontend/`) is
embedded in the binary at compile time. Building with the `web` feature
(the default) therefore requires Node once, to produce the bundle:

```sh
cd crates/pg_lens_web/frontend && npm ci && npm run build
```

Skip the requirement entirely with a TUI-only build:
`cargo build -p pg_lens_tui --no-default-features`. Release binaries from
CI always ship with the web dashboard included.

### Authentication

Binding to anything other than loopback requires a token:

```sh
PG_LENS_AUTH_TOKEN=$(openssl rand -hex 32) pg_lens serve --listen 0.0.0.0:8080
```

All `/api/*` routes then require `Authorization: Bearer <token>` — the web
page prompts for the token on first load. Because the browser `EventSource`
API cannot set headers, `/api/*` also accepts `?token=<token>` as an
equivalent credential (same constant-time comparison). Trade-off: a token
in a URL can end up in reverse-proxy access logs and browser history —
treat it as a revocable secret, rotate it by restarting with a new value,
and never expose the server without TLS.

### Security notes

- **Read-only DSN**: connect with a role granted only
  [`pg_monitor`](https://www.postgresql.org/docs/current/predefined-roles.html)
  (`CREATE ROLE lens LOGIN PASSWORD '...'; GRANT pg_monitor TO lens;`) —
  the dashboard needs nothing more.
- **TLS**: the binary does not terminate TLS; put a reverse proxy (Caddy,
  nginx) in front for any non-localhost deployment.
- **Default bind is `127.0.0.1`** — exposure is an explicit operator
  decision, and refused without `PG_LENS_AUTH_TOKEN`.
- The DSN/password never appears in any endpoint, log line, or JSON payload.

## Roadmap

- [x] **Web Lens** — `pg_lens serve`: an axum-based web dashboard consuming
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
