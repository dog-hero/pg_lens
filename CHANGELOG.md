# Changelog

All notable changes to pg_lens. Format inspired by
[Keep a Changelog](https://keepachangelog.com); versions follow
[SemVer](https://semver.org). Dates are release dates.

## [Unreleased] — v0.8 "Room to breathe" (in progress)

### Added
- **Index Lens** — the index advisor promoted to its own tab (full-height
  table, `Enter` detail with indexdef + duplicate partner); the `i` toggle
  inside Schema Lens is gone.
- **Replication Lens** — dedicated tab with every WAL sender/receiver and
  **all** replication slots as a scrollable table (worst severity first);
  the Macro Lens panel stays as the compact summary with a
  "Tab → Replication for all" hint when it clips.
- Web: top-level tabs now mirror the TUI (Activity │ Replication │ Schema │
  Indexes │ Queries).
- **Database selector** — `d` opens a picker of the cluster's databases
  (name + size, current marked); selecting reconnects the poller to the
  chosen database with per-database state (schema/queries/history) reset.
  The header now names the current database. Web switching deferred (the
  web follows the poller's database).

### Planned
- Waits & vacuum visibility (`w` full waits panel; vacuum sub-view with room).

## [0.7.1] — 2026-07-16

### Fixed
- `pg_lens serve` no longer hangs on Ctrl+C while SSE clients (browser
  tabs) are attached — the poller now shuts down inside axum's
  graceful-shutdown window, closing the streams that shutdown was waiting on.
- Replication slots are no longer pushed out of the Macro Lens panel by
  many active WAL senders: senders cap at 4 with "… +N more", slots keep
  their own section ranked worst-first.
- Double-`Esc` quit barrier: a stray/hammered `Esc` closing overlays never
  exits the app anymore — a hint appears and only a second `Esc` within ~2s
  quits (`q`/Ctrl+C stay immediate).

## [0.7.0] — 2026-07-16 — "What should I go fix"

### Added
- **Top waits strip** (Micro Lens): live ranked aggregation of
  `wait_event_type:wait_event` across all sessions — Lock:* red, IO:* yellow.
- **Vacuum health & XID wraparound**: cluster `age(datfrozenxid)` headline
  (yellow >200M, red >500M xids) with a Macro Lens banner past thresholds,
  worst tables by age + dead-tuple ratio, and live
  `pg_stat_progress_vacuum` progress.
- **Replication slots view**: `pg_replication_slots` (both roles) with
  retained WAL, `wal_status`, `safe_wal_size`; inactive-retaining slots
  yellow, `unreserved`/`lost` red — the classic full-disk incident, visible.
- **Index advisor**: unused (constraint indexes never flagged),
  exact-duplicate and prefix-redundant index detection with sizes, scans
  and stats-reset age — signal, not verdict.
- **Checkpointer / bgwriter panel**: checkpoints/min (timed vs requested),
  buffers/s by source, avg write/sync time; checkpoint-pressure warning
  when requested > timed. Version-gated across the PG 17 catalog split.

All five in both the TUI and the Web Lens.

## [0.6.1] — 2026-07-15

### Fixed
- Quitting pg_lens now cancels its in-flight query server-side
  (CancelRequest) — a heavy on-demand bloat estimate no longer keeps
  running after exit until `statement_timeout`.

## [0.6.0] — 2026-07-15

### Added
- **Activity filter**: `/` in the Micro Lens filters live by pid, db, user,
  application, client, state, wait or query text (`Enter` applies, `Esc`
  reverts); a search box mirrors it in the web.
- **Persistent history**: the TPS/sessions chart survives restarts (JSONL
  per connection target under the XDG state dir); ring capacity raised to
  1 hour at the default 2s tick.
- **config.toml**: persistent defaults for `interval`, `schema_interval`
  and `listen` (`~/.config/pg_lens/config.toml`; precedence
  flag → env → config → default).
- **Web parity**: pause button, schema/bloat refresh button, and
  cancel/terminate actions in the web — admin strictly requires
  `PG_LENS_AUTH_TOKEN` (403 otherwise).
- Empty-state messages for the activity table ("no sessions match" vs
  "no active sessions").

## [0.5.3] — 2026-07-15

### Changed
- Every poll now runs inside a per-tick **read-only transaction** with
  `SET LOCAL statement_timeout` — pooler-safe (prepare + execute on one
  backend), a consistent MVCC snapshot per tick, and a hard safety ceiling
  for every query. The poller session identifies itself
  (`application_name = 'pg_lens'`).

### Fixed
- Connection flags (`--service`, `--dsn`, …) are now global: `pg_lens
  --service X serve` no longer silently ignores the service.

### Known limitation
- PgBouncer *transaction* pooling remains unsupported (named prepared
  statements leak across backends); use session pooling or a direct
  connection — documented in the README.

## [0.5.2] — 2026-07-15

### Fixed
- Poll no longer dies on restricted/managed servers (RDS, Cloud SQL, …):
  replication/WAL queries are best-effort — a denied view degrades to an
  absent panel instead of a dead poll; poll errors now carry the real
  PostgreSQL message + SQLSTATE.
- The Homebrew cask clears the Gatekeeper quarantine itself (`postflight`)
  — Homebrew removed the `--no-quarantine` flag.

## [0.5.1] — 2026-07-15

### Fixed
- Connecting is instant again: the slow schema collection no longer blocks
  the first snapshot, and estimated bloat is **on-demand** (`R` in the
  Schema Lens) — the ioguix bloat queries are too heavy for the auto cadence.

## [0.5.0] — 2026-07-15

### Added
- **Query Lens**: `pg_stat_statements` top statements (calls, total/mean
  time, rows, hit%) with a friendly explainer when the extension is
  missing/old.
- **Replication/WAL panel**: `pg_stat_replication` (primary) /
  `pg_stat_wal_receiver` (standby) with tiered lag severity.
- **Admin actions**: `c` cancel / `K` terminate with a confirmation modal
  (TUI only at the time).
- **Pause**: spacebar freezes the view for point-in-time analysis.
- Web SQL syntax highlighting.
- Published to **crates.io** (`cargo install pg_lens_tui`, binstall
  metadata included).

## [0.4.0] — 2026-07-15

### Added
- Interactive service picker on startup (no flags + valid services file).
- TUI polish pass; demo gif + web screenshot.

## [0.3.0] — 2026-07-14

### Added
- **Schema Lens**: `pg_stat_user_tables` + on-disk sizes on a slow cadence,
  estimated table/index bloat (ioguix queries, BSD-2-Clause attribution),
  severity markers.

## [0.2.x] — 2026-07-14

### Added
- Binary renamed to `pg_lens`; Homebrew tap (formula + cask), Docker/GHCR
  image, deb/rpm packages — full distribution pipeline from one tag push.
- Advanced connections: libpq env vars, `services.toml` with
  `password_cmd` (secrets from vault/keychain, never in the file).

## [0.1.0] — 2026-07-14

### Added
- MVP: Macro Lens (vitals, TPS/sessions sparklines) + Micro Lens (activity
  with blocked/waiting markers, detail panel), real data layer with
  version-gated SQL (PG 13+), resilient poller (reconnect + last-good-data
  banner), mock mode, PTY e2e harness.
- **Web Lens**: `pg_lens serve` — axum + SSE streaming the same snapshots,
  embedded TypeScript frontend, bearer-token auth, non-loopback bind
  refused without a token.
