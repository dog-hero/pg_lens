# Changelog

All notable changes to pg_lens. Format inspired by
[Keep a Changelog](https://keepachangelog.com); versions follow
[SemVer](https://semver.org). Dates are release dates.

## [0.13.0] — 2026-07-17

### Navigation & filters
- **Direct tab jump (`1`–`6`)** — jump straight to any lens by number; the
  tab bar now shows the digit prefix for each (`1 Macro │ 2 Micro │ …`) so
  the binding is self-documenting.
- **`Shift+Tab` backward cycle** — `Tab::prev()` was previously unwired;
  `BackTab` now cycles lenses in reverse.
- **`Backspace` last-tab toggle** — browser-back-style jump to the
  previously active lens (still deletes as expected inside a filter
  editor).
- **Fast scroll everywhere** — `Home`/`g` and `End`/`G` jump to the first/
  last row, `PageUp`/`PageDown` move by a page, on every selectable table
  in the TUI.
- **Schema and Query Lens filters (`/`)** — the textual filter, previously
  Micro-Lens-only, now also filters the Schema Lens (Tables view, by
  schema/table name) and the Query Lens (by query text), each with its own
  independent filter state; `\` clears whichever lens's filter is active.
  Web parity: `#schema-filter` / `#statements-filter` search inputs.
- All of the above added to the `?` help overlay.

### Web catch-up & redesign
- **Modern dashboard redesign** — new layout: a left sidenav (collapsing to
  an icon rail ≤1024px, a horizontal strip ≤720px) and a redesigned topbar
  (brand, current database + switcher, read-only badge, pause, connection
  state, theme toggle). Inline SVG icon sprite, no icon font or UI
  framework added. System-UI font for chrome, monospace for data. The
  Checkpoints card now leads with a `X.XX/min` headline and pushes detail
  numbers below it. Bundle grew 116→132 KB (still embedded, no new
  runtime dependency).
- **Light/dark theme toggle** — a header toggle persisted in
  `localStorage`; defaults to dark (unchanged behavior for existing users).
- **Web keyboard navigation** — `1`–`5` jump to the nav sections, `/`
  focuses the active panel's filter input, `Esc` blurs it; shortcuts are
  suppressed while typing in a text input (except `Esc`).
- **Web database switcher** — a header dropdown lists every database the
  connected role can see and switches the poller to it via the new
  `POST /api/db/switch {"database": "name"}` endpoint (token-gated the same
  way as `/api/schema/refresh` — a database switch is a read-only
  reconnect, so it works even under `--read-only`; the target is validated
  against the snapshot's `databases` list, `400` on an unknown name). The
  poller's existing db-switch channel — previously dropped in `run_serve` —
  is now threaded into `WebState`. Degrades to just showing the current
  database name when `databases` is null or has fewer than two entries.
- **`serve` fail-loud on ambiguous services** — `pg_lens serve` with a
  `services.toml` defining one or more services and no `--service`/`--dsn`/
  env var previously fell through and silently connected to `localhost`
  (the TUI shows an interactive picker in this situation, but `serve` has
  no TTY to show one to). It now lists the available service names
  (host/user, never secrets) to stderr and exits non-zero instead.

### Fixed
- **`--dsn` / `--service` conflict across the `serve` subcommand
  boundary** — both flags are declared `global`, which let clap's
  `conflicts_with` be silently bypassed when one flag was given before
  `serve` and the other after; pg_lens would connect to `--dsn` and
  quietly ignore `--service`, a wrong-server footgun. Fixed with a runtime
  backstop, `ConnArgs::ensure_conn_flags_consistent()`, called right after
  argument parsing regardless of flag position.

## [0.11.0] — 2026-07-17 — "Incident precursors & connection visibility"

### Added
- **Idle connection / connection-age census** — the Micro Lens `I` key toggles
  the activity table to a dedicated idle-session view (`state = 'idle'`,
  oldest-first, capped at 100): PID, age, user, database, application, and
  client address, with a headline ("N idle connections, oldest …") and
  yellow/red age tiers (30 min / 4 h). Solves the classic pool-exhaustion
  incident — many connections used but few active — where the existing
  activity table filters idle sessions out entirely. `Esc` closes it;
  `/`/`w`/`c`/`K`/`s` are inert while it's open. Best-effort, fast tick.
  TUI + Web.
- **Lock-table pressure gauge** — a third gauge in the Macro Lens vitals
  strip: held locks vs. capacity (`max_locks_per_transaction ×
  (max_connections + max_prepared_transactions)`), yellow at 60%, red at
  85%, warning before "out of shared memory, you might need to increase
  max_locks_per_transaction". Best-effort, fast tick. TUI + Web.
- **Invalid / not-ready index flag** — the Index Lens now flags indexes left
  behind by a failed `CREATE INDEX CONCURRENTLY` (`pg_index.indisvalid` /
  `indisready`) as a new `INVALID` finding, ranked ahead of `UNUSED`, with
  actionable detail text (drop and rebuild). TUI + Web.
- **Open a `psql` shell from pg_lens** (`!`, TUI-only) — suspends the TUI and
  spawns `psql` on the exact connection pg_lens is polling (host/port/user/
  dbname), restoring the terminal on exit, spawn failure, or `psql` missing
  from `PATH`. The password is never passed on the command line — it's
  resolved as late as possible and handed to the child only via a
  `PGPASSWORD` environment variable. Under `--read-only`, the shell launches
  with `PGOPTIONS=-c default_transaction_read_only=on` and prints a notice
  that this is a default, not a hard sandbox — a full `psql` session can
  still override it explicitly. Disabled (with a clear message) in `--mock`,
  since there's no real connection to hand psql.

## [0.10.0] — 2026-07-16

### Added
- **Read-only mode** — a `--read-only` flag / `PG_LENS_READ_ONLY` env var /
  `read_only = true` in `config.toml` (precedence: flag → env → config →
  default `false`) hard-disables every admin/mutating action for
  shared or audited deployments. This is a real server-side gate, not UI
  hiding: in the TUI, `open_confirm()` refuses `c`/`K` *before* the confirm
  modal ever opens (inline "read-only mode — action disabled" feedback,
  plus a permanent yellow `RO` marker in the header so the mode is never
  silently active); in the Web Lens, the `/api/admin/*` endpoints return
  `403` even when a valid `PG_LENS_AUTH_TOKEN` is presented. A new
  `GET /api/config` endpoint exposes `{"read_only": bool}` and the web
  frontend disables the cancel/terminate buttons and shows a badge to
  match. Schema refresh (`R`) is unaffected — it only ever opens a
  read-only transaction. `pg_lens serve` inherits the same flag/env/config
  resolution. TUI + Web.
- **Remote connection config** — `--config-url <URL>` / `PG_LENS_CONFIG_URL`
  env / `remote_config` in `config.toml` loads a shared `services.toml`
  from a remote source, so a team can point every machine at one curated
  target list instead of copying the file by hand. Accepts either a
  `github:OWNER/REPO/PATH[@REF]` shorthand (fetched via the GitHub
  Contents API) or a verbatim `https://`/`http://` URL. The token is
  never stored in a file: it comes from `PG_LENS_CONFIG_TOKEN`, then
  `GITHUB_TOKEN`, then a `remote_config_token_cmd` in `config.toml`
  (mirrors the existing `password_cmd` pattern — an external command,
  trimmed stdout), sent as `Authorization: Bearer`; a token is refused
  outright over plain `http://`. A successful fetch is cached at
  `$XDG_CACHE_HOME/pg_lens/remote-services.toml` (mode `0600`); a failed
  fetch falls back to that cache, then the local services file, with a
  stderr warning at each step — startup never blocks on a flaky network
  (10s timeout) and never hard-fails while a local or cached file can
  still serve. Remote entries win on a same-named collision with local
  entries; the fetch is strictly read-only, pg_lens never writes back to
  the remote source.

## [0.9.0] — 2026-07-16 — "Problem transactions"

The cheap, cohesive batch around long/idle transactions and blocking —
every item reuses data already polled from `pg_stat_activity`, plus a
keyboard help overlay that also clears the stale-README debt.

### Added
- **Idle-in-transaction / transaction-age hunter** — a new transaction-age
  column and marker in the Micro Lens, plus a headline for the oldest
  `idle in transaction` / long-running transaction (the session driving
  XID-wraparound risk and lock retention). Yellow/red age tiers (idle-in-tx
  escalates earlier than a plain long-running transaction). TUI + Web.
- **Blocking chain / lock-wait graph** — the Micro Lens detail panel for a
  selected PID now renders the full wait-for chain (`A→B→C`) to the root
  blocker, with the root highlighted and a warning if a deadlock cycle is
  detected. Reuses the `blocked_by` data already polled. TUI + Web.
- **Prepared-transaction (orphaned 2PC) watch** — orphaned two-phase
  commits (`pg_prepared_xacts`) hold locks and block vacuum forever; they
  now show up (gid, age, owner, database) inside the Vacuum sub-view, with
  yellow/red age tiers and a best-effort absent panel when none exist or
  the view isn't visible. TUI + Web.
- **Keyboard help overlay (`?`)** — a static reference listing every
  binding, grouped by navigation / sub-views / data & refresh / admin /
  quit; doubles as the source of truth the README keybindings table is
  reconciled against.
- **Docs: connection user & least-privilege** — a new
  [`docs/connection-user.md`](docs/connection-user.md) page (linked from
  the README) covering how to create the monitoring role and the exact
  grants each lens needs, what degrades to an absent panel without them,
  and the read-only posture.

## [0.8.0] — 2026-07-16 — "Room to breathe"

Give the v0.7 data room to breathe: dedicated tabs, scrollable lists,
per-database navigation.

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
- **Full waits panel** — `w` in the Micro Lens opens the complete ranked
  wait list (count, % of waiting sessions, proportional bar), beyond the
  one-line strip. Web: a collapsible waits list under the activity table.
- **Vacuum sub-view** — `v` in the Schema Lens switches to a full-height
  vacuum view: cluster wraparound headline, all worst tables by XID age
  (scrollable, with dead-tuple ratio and last-vacuum age), and live
  `pg_stat_progress_vacuum`. Web renders the full worst-tables list.

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
