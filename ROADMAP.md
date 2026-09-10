# pg_lens — Roadmap

Execution order for [PRD.md](PRD.md). Sourced from the `feature-discovery`
agent's 2026-07-15 research (comparables: pg_activity, pghero, pganalyze,
pgcenter, pg_top) plus the owner's priorities. Every item follows the PRD's
Definition of Done. Check items off as they ship; move them to the Shipped
section on release.

---

## v0.9 — "Problem transactions" (shipped — see Shipped section)

The cheap, cohesive batch around long/idle transactions and blocking — every
item reuses data already polled (`pg_stat_activity`), plus the help overlay
that also clears the stale-README debt.

- [x] **Idle-in-transaction / transaction-age hunter** — surface
  `pg_stat_activity.xact_start` (already in the activity poll, one new column):
  a transaction-age column + marker in the Micro Lens, and a headline for the
  oldest `idle in transaction` / long-running xact (the session driving
  XID-wraparound risk and lock retention). Yellow/red age tiers. TUI + Web.
- [x] **Blocking chain / lock-wait graph** — reuse the `blocked_by` arrays
  already returned (no new SQL): render the wait-for chain (A→B→C) in the
  Micro Lens detail panel for the selected PID, root blocker highlighted;
  watch for deadlock cycles. TUI + Web.
- [x] **Prepared-transaction (orphaned 2PC) watch** — `pg_prepared_xacts`:
  orphaned two-phase commits hold locks and block vacuum forever; show them
  (gid, age, owner, database) inside the Vacuum sub-view. Best-effort absent
  panel when empty/unavailable. TUI + Web.
- [x] **Keyboard help overlay (`?`)** — static overlay listing every binding
  (no data source); doubles as the single source of truth the README
  keybindings table is reconciled against.
- [x] **Docs: connection user & least-privilege** — a `docs/` page (linked
  from the README) on how to create the monitoring role and the exact grants
  each lens needs (`pg_monitor` vs. explicit `GRANT`s: `pg_stat_activity`
  full rows, `pg_stat_statements`, replication views, `pg_stat_progress_*`),
  what degrades to an absent panel without them, and the read-only posture.

## v0.10 — shipped (see Shipped section)

## v0.11 — "Incident precursors & connection visibility" (shipped — see Shipped section)

Cheap, cohesive batch from the 2026-07-17 discovery: close an outright
visibility hole (idle connections), add two leading-indicator gauges that warn
*before* an outage, and let the operator jump straight to a `psql` shell from
the session they're staring at. Every data item reuses a view already polled or
a one-column catalog read.

- [x] **Idle connection / connection-age census** — the Micro Lens activity
  query filters out idle sessions (`WHERE state <> 'idle'`), so the classic
  pool-exhaustion incident (`connections_total` near `max_connections` but few
  active) is undiagnosable today. Surface idle sessions ranked by age
  (`now() - state_change`), with `application_name` / `client_addr` / `usename`
  / `datname` — all columns already selected for active rows, same
  `pg_stat_activity`, PG 13+. A toggle/second panel in the Micro Lens (reuse
  the activity table component). TUI + Web. **S/M.**
- [x] **Lock-table pressure gauge** — headroom before "out of shared memory,
  you might need to increase max_locks_per_transaction". `count(*) FROM
  pg_locks` vs. `max_locks_per_transaction × (max_connections +
  max_prepared_transactions)` (documented capacity formula) — cheap aggregate
  + scalar settings, PG 13+. A yellow/red gauge in the Macro Lens vitals strip
  (reuse existing severity-tier styling). TUI + Web. **S.**
- [x] **Invalid / not-ready index flag** — indexes left behind by a failed
  `CREATE INDEX CONCURRENTLY` waste write I/O and disk while serving no query;
  `\d` never warns. `pg_index.indisvalid` / `indisready` — the join already
  exists in `indexes.sql`, one more column + one advisor category in the Index
  Lens (`index_advisor::classify`). Best-effort. TUI + Web. **S.**
- [x] **Open `psql` from pg_lens** — jump from the session you're inspecting
  straight into a `psql` shell on the same connection. TUI keybinding (suggest
  a mnemonic like `!` or `p`): suspend the alternate screen / raw mode, spawn
  `psql` reconstructing the resolved connection params (host/port/user/dbname;
  pass the password via a transient `PGPASSWORD` in the child env or a libpq
  `passfile`, never on the argv/command line), restore the TUI cleanly on exit.
  Design/open questions to settle at build time: (a) locate `psql` on `PATH`,
  degrade to a clear message if absent — pg_lens must not require psql to run;
  (b) **read-only mode interaction** — `--read-only` gates pg_lens's OWN admin
  actions, but a psql shell is full access; decide whether read-only disables
  the launch, warns, or passes `-v` / a read-only-transaction default — at
  minimum surface that psql is unrestricted; (c) `serve`/web has no local
  terminal, so this is **TUI-only** (no web surface); (d) restore terminal
  state even if psql crashes (RAII/guard around the suspend). Secret handling
  is the sharp edge — treat it like `password_cmd`: resolve late, keep it out
  of argv, logs, and history. TUI only. **M.**

Deferred candidate from the same pass (map, don't build this batch):
- **Query I/O & temp-spill profile** — `pg_stat_statements` `temp_blks_*`,
  `shared_blks_dirtied/written`, `blk_read_time`/`blk_write_time` (gated on
  `track_io_timing`), `wal_bytes` (ext ≥1.9); pure column addition to the
  existing Query Lens table/detail, mirroring the existing optional-timing
  version gate. The #1 query-tuning signal the Query Lens still lacks. **S/M.**

## v0.13 — "Web catch-up & redesign" (shipped — see Shipped section)

The Web Lens is actually near-feature-parity already (the 2026-07-17 audit found
almost every TUI feature mirrored), but it *feels* stale: no keyboard nav, no
database switcher, and a dated look. Close the real gaps and remodel the visual.
Owner picks: **modern observability dashboard** direction; **DB switch + serve
fail-loud** scope.

- [x] **Web database switcher + `serve` fail-loud** — the poller's DB-switch
  channel (`mpsc<String>`) is fully wired and frontend-agnostic but `run_serve`
  *drops the sender* (`main.rs`, documented v0.8 deferral). Thread `db_switch_tx`
  into `pg_lens_web::WebState`, add `POST /api/db/switch` (alongside
  `/api/schema/refresh` — a DB switch is a read-only reconnect, safe even in
  read-only mode, NOT an admin action), declare `databases` in `types.ts` (data
  already streams in `/api/snapshot`), and put a database dropdown in the header.
  Separately, **`serve` with a services file and no `--service`/`--dsn`/env must
  fail loud** — list the available service names and refuse, instead of silently
  connecting to the localhost default (current footgun). **M + S.**
- [x] **Web keyboard navigation** — the web is 100% click-driven; add a
  `keydown` dispatcher: `1`–`5` jump to the nav tabs, `/` focuses the active
  panel's filter input, `Esc` blurs. Pure frontend, no API change (every element
  already has an id). Fold into the redesign build. **S.**
- [x] **Modern observability dashboard redesign** — remodel the frontend
  (`crates/pg_lens_web/frontend/`) into a modern dashboard: responsive
  multi-column grid, inline hand-picked SVG icons (no icon font/framework —
  bundle stays lean, `dist/` is embedded via rust-embed and currently ~116 KB),
  a **light/dark toggle** (second CSS-variable set; keep severity warn/bad
  legible in both), a clearer header carrying the connection target + database
  name + the new switcher + RO badge + pause/conn-state, and a few well-chosen
  small charts reusing the existing uPlot dep. Keep severity colors consistent
  with the TUI. No new runtime framework; must not bloat the embedded bundle.
  Absorbs the DB switcher and keyboard nav into the new chrome. **L.**

## v0.12 — "Navigation & filters" UX polish (shipped in v0.13.0 — see Shipped section)

Fast-wins usability batch from the 2026-07-17 polish discovery. No new data
sources — pure interaction/ergonomics, TUI-first with web parity where cheap.

- [x] **Group A — navigation & scroll**: direct tab jump with `1`–`6`;
  `BackTab` (Shift+Tab) backward cycle (currently unwired — `Tab::prev()`
  missing); a "last tab" toggle (`Backspace`, browser-back style, stores
  `previous_tab`); number prefixes in the tab bar (`1 Macro │ 2 Micro │ …`) so
  the digit binding is self-documenting; and fast scroll on every long table —
  `Home`/`End`/`PageUp`/`PageDown` (+ vim `g`/`G`), reusing `move_selection`'s
  existing arbitrary-delta support. Add the new keys to the help overlay.
- [x] **Group B — lens filters**: a textual `/` filter on the Schema Lens
  (Tables view — by schema and table name) and the Query Lens (by query text),
  mirroring the Micro Lens activity-filter interaction with **per-lens** filter
  state (not a shared generic field); plus a one-key clear-filter (when a
  committed filter is non-empty and not editing). Web parity: a search box on
  the Schema and Queries tabs, same shape as the existing activity filter.
  Index Lens deliberately excluded (few rows). Add the keys to the help overlay.

Explicitly NOT doing (discovery correction): `s` sort-cycle on Index/Replication
Lens — those are intentionally fixed severity-ranked order, not a gap.

## v0.14 — "See the trend, not just the moment" (shipped — see Shipped section)

The persisted 1h history (JSONL ring, survives restarts) today carries only
`tps` + `active_sessions`; every lens is otherwise point-in-time. This batch
turns the history into pg_lens's differentiator (pg_activity/pgcenter are pure
point-in-time; trend charts are pghero/pganalyze's headline). Plus one
independent Query Lens win as a hedge.

- [x] **Widen `SnapshotHistory` + vitals trend arrows** — add per-tick scalars
  already computed by the poller (lock-pressure %, oldest-XID age, connections,
  cache-hit) to `HistoryPoint` with `#[serde(default)]` (old JSONL keeps
  loading). Trend arrows (↑/↓/→ vs ~5 min ago) on the Macro Lens vitals cards;
  web mirrors with a tooltip delta. No new SQL. Foundational — every future
  trend feature becomes an S extension. **S/M.**
- [x] **History time-scrubber (web)** — drag over the history chart to pin a
  moment and read the vitals as they were then (incident review). Reuses the
  widened history already streamed over SSE + uPlot's cursor API. Web-only. **S.**
- [x] **Table/index size growth (Schema Lens)** — "this table grew 40% in the
  last hour": a Δsize(1h) column from a bounded per-table ring (cap top-N by
  size, evict on schema refresh — never unbounded). Slow cadence only. TUI +
  Web. **M.**
- [x] **Query I/O & temp-spill profile** (deferred from v0.11) —
  `pg_stat_statements` `temp_blks_read/written`, `shared_blks_dirtied/written`,
  `blk_read_time`/`blk_write_time` (gated on `track_io_timing`, mirroring the
  checkpointer's optional-timing pattern), `wal_bytes` (ext ≥1.9). Pure column
  addition to the Query Lens table/detail. TUI + Web. **S/M.**

Also in tree (unreleased, built 2026-07-17 from owner feedback): interactive
service picker for `pg_lens serve` — TTY prompt with a numbered list when a
services file exists and nothing was selected (auto-select with notice when
exactly one); non-TTY keeps the v0.13 fail-loud.

## v0.15 — "Schema Lens completo" (shipped — see Shipped section)

From the 2026-08-05 Schema-Lens-focused discovery, seeded by owner reports.
Two of the three reports were confirmed as real defects/gaps with code evidence.

- [x] **Honest table counts + limit fix (BUG)** — `table_stats` SQL has a
  silent `LIMIT 200` (by size desc): table 201+ never appears and the footer
  shows the truncated count as if it were the total. Also a perf trap: the
  `ORDER BY pg_total_relation_size(...)` evaluates size for EVERY table in the
  cluster even though only 200 survive. Fix: footer shows `N of M tables`
  (real total via a cheap `count(*)`), make the limit configurable
  (config.toml + flag), and restructure the query so per-row size evaluation
  is bounded (subquery-limit by a cheap ordering first, or document the cost).
  TUI + Web. **S/M.**
- [x] **Table structure detail (on-demand)** — Enter on a table today shows
  sizes/tuples/vacuum/bloat/indexes only. Add the table's STRUCTURE — the
  kind of information psql's `\d` covers (columns with name/type/nullable/
  default; constraints PK/FK/UNIQUE/CHECK via `pg_get_constraintdef`;
  referencing FKs "what references this table"; index definitions) — but
  rendered in pg_lens's own detail-panel style, NOT a psql-output clone.
  Fetched ON-DEMAND when the detail opens (new request/response channel
  following the `AdminCommand` immediate-wake pattern; never on the poll
  cadence). Scrollable detail overlay. TUI + Web. **M/L.**
- [x] **Partition collapsing + drill-down** — native partitioned tables:
  parents (`relkind='p'`) have no `pg_stat_user_tables` row and every leaf
  partition floods the list. Collapse to the PARENT by default with
  aggregated stats (sum sizes/rows/dead via `pg_partition_tree`, partition
  count column); Enter on a parent drills into its partitions. Growth ring
  re-keys to the parent oid for aggregated Δ1h. TUI + Web. **L.**
- [x] **Cross-lens jump to Query Lens + table lock indicator** — a key on the
  selected table (suggest `x`) jumps to the Query Lens with
  `statements_filter` seeded to the table name (substring match — imperfect,
  documented); plus a lock-count indicator per table joining the
  already-polled `pg_locks` data (no new SQL) so a locked table is visible
  without tab-switching. TUI + Web where cheap. **M.**

Deferred from the same pass: Index Lens jump (needs new filter state —
reopens a v0.12 decision; after the Query jump proves the pattern), sequence
exhaustion, matview badge / TOAST split / per-table cache-hit columns.

## Project site — GitHub Pages (shipped 2026-08-05)

Landing page + docs + a **live interactive demo** at
`https://dog-hero.github.io/pg_lens/`. The Web Lens frontend is a static bundle
that only consumes JSON, so the real dashboard can run on Pages against canned
snapshots captured from `--mock` — visitors click through all six lenses with
no backend. Deployed by a `pages.yml` workflow (`actions/deploy-pages`);
independent of the release pipeline and of the binary-embedded bundle.

- [x] **Demo build mode** — a second frontend build (`dist-demo/`, base
  `/pg_lens/demo/`) whose entry stubs `EventSource`/`fetch` over committed
  snapshot fixtures generated from `pg_lens --mock serve`. Must not alter the
  production build embedded via rust-embed (absolute `/assets/` at site root).
  Visible "demo — canned data" badge; POST actions no-op with a toast.
- [x] **Landing page** — static HTML/CSS reusing the v0.13 design tokens
  (dark/light, severity colors, inline SVG icons): hero with the demo gif,
  CTAs (live demo / install / GitHub), feature tour, install tabs
  (brew/cargo/binaries), docs links.
- [x] **Docs pages** — render `docs/connection-user.md` and `CHANGELOG.md` to
  styled HTML at build time (build-time devDependency only — never in the
  embedded bundle).
- [x] **`pages.yml` workflow** — build demo + landing, assemble `_site/`,
  `upload-pages-artifact` + `deploy-pages` on push to main + manual dispatch.
  Repo setting required: Settings → Pages → Source: GitHub Actions.

## v0.16 — "First impression" (shipped — see Shipped section)

Onboarding + the two adjacent stat views a fresh install benefits from
immediately, plus two owner-requested UX items.

- [x] **`curl | sh` install script** — `scripts/install.sh`: platform detection
  (macOS arm64/x86_64, Linux musl x86_64/aarch64 — musl means no libc
  detection), latest-version resolution via the GitHub API with
  `PG_LENS_VERSION`/`--version` pinning, **SHA-256 verification against the
  `.sha256` sidecars the release already publishes** (no release.yml change
  needed), `~/.local/bin` default with no sudo and no rc-file editing,
  `--dry-run`/`--help`, re-run-to-upgrade, loud failure on unpublished
  platforms. Mirrored to the Pages site as a build-time copy.
- [x] **Micro Lens row colors (pg_activity-style)** — whole-row color: one
  color per session state (active/idle/idle-in-tx/aborted), overridden by
  query duration (>30s red, >10s orange) **only for `active` rows** so an old
  idle session is never mislabelled; blocked outranks both. Selection stays
  unmistakable. SQL keyword highlighting moved out of the table row and kept
  in the expanded detail. TUI + Web.
- [x] **Copy to clipboard** — `y` copies the selected item (Micro: full query;
  Query Lens: statement; Index Lens: `CREATE INDEX`; Schema: qualified name)
  via **OSC 52**, which works over SSH and needs no dependency; the toast is
  honest that the terminal may still refuse (Apple Terminal does not support
  it). Web: copy buttons on the query details.
- [x] **I/O profile — `pg_stat_io` (PG 16+)** — aggregated by
  backend_type × context, all-zero rows filtered, per-second rates derived
  from deltas with reset handling, timing `--` when `track_io_timing` is off.
  Slow cadence; absent (not broken) on PG 13–15. Lives in the Macro Lens's
  buffer/IO column, no seventh tab.
- [x] **WAL generation rate (`pg_stat_wal`, PG 14+)** — WAL bytes/s and
  records/s from deltas, plus `wal_buffers_full` tinted only while actively
  climbing (a tuning nudge, not an incident). Fast tick (one tiny row, and the
  rate is genuinely spiky). Replication Lens panel + a Macro Lens line;
  absent on PG 13.
- [x] **Shell completions** — `pg_lens completions <bash|zsh|fish|powershell|
  elvish>` via `clap_complete`, handled before any DB/terminal work.
  Documented in the README and suggested by the installer's post-install
  banner.

## v0.17 — "Blocks & Locks Lens" (shipped — see Shipped section)

Dedicated locks inspection lens plus adjacent connection security and maintenance progress tracking.

- [x] **Blocks & Locks Lens** — wait-for tree + active locks table (`pg_locks`),
  session cancellation/termination directly from tree or table, positioned in
  tab 3 between Micro Lens and Replication. TUI + Web.
- [x] **DDL progress** — `pg_stat_progress_create_index`, `_analyze`, `_basebackup`
  monitoring in session detail and Web maintenance panel.
- [x] **SSL/TLS connection security** — `pg_stat_ssl` cipher and protocol
  version in activity tables, idle census, and session detail.

## v0.8+ candidates (from the discovery research — re-rank before starting)

(All prioritized candidates shipped!)

## Backlog (deliberately deprioritized — owner decision 2026-07-15)

- pg_service.conf / .pgpass compatibility (C3)
- Apple notarization (D5; removes the cask quarantine postflight, needs a
  paid Apple Developer account)
- Prometheus `/metrics` export
- Multi-instance monitoring (N servers, one screen)
- Docker/GHCR image re-enable (one-line revert in release.yml; move to a
  native arm64 runner first)
- PgBouncer *transaction* pooling support (requires a `simple_query`
  protocol rewrite; session pooling and direct connections work today)

## Shipped

- **v0.22.0** — "Dual-Lane Poller Architecture, Fast Incident Path & Sub-Second Precision":
  - **Dual-Lane Poller Architecture**: dedicated Fast Lane (`client_fast`) with 1-RTT transaction pipelining for incident queries (`activity`, `blocking`, `server_info`, `bgwriter`, `active_locks`, `locks_by_relation`) reducing RTT from ~39 to 1; independent Telemetry Lane (`client_telemetry`) decoupling slower catalog queries into Tiers 2/3/4 (3s, 30s, 60s) with atomic state synthesis via `Arc<RwLock<SharedTelemetry>>`.
  - **Single Connection Mode**: `--single-connection` CLI flag and auto-fallback when connection limits or poolers prevent secondary connections.
  - **Activity State Column Placement**: moved `State` column after `Xact` in TUI and Web table views for tighter operational context.
  - **Sub-Second & Sub-Millisecond Precision**: 4-decimal precision formatting for execution times `< 1s` (`0.0001s`) and `< 1ms` (`0.0500ms`) across TUI and Web.
- **v0.21.0** — "Web Modernization, 9-Lens Parity & Runtime Cluster Switching":
  - **Runtime Server / Cluster Switching**: dynamic runtime server switching (`services.toml`) in TUI (`C` server picker modal) and Web UI (`#server-target-group`, `GET /api/servers`, `POST /api/server/switch`, Command Palette "Servers" category) with safe cancellation, state reset, and immediate zero-backoff reconnect.
  - **Web Dashboard Modernization**: vertical space overhaul with dedicated Macro Lens (Lens 1) cockpit, compact single-line health ribbon on Lenses 2–9, and 100% viewport height for sticky data tables.
  - **Slide-Over Inspector Drawer**: smooth slide-over drawer replacing accordion rows with dedicated panels for Sessions, Blocking Chains, Schema Tables, Replication Slots, and Statements.
  - **Quick Command Palette**: `Cmd+K` / `Ctrl+K` global palette for fuzzy navigation across lenses, databases, servers, and incident actions.
  - **Full 9-Lens Parity & Modular CSS**: all 9 lenses in Web UI matching TUI capabilities, powered by a clean tokenized CSS architecture.
- **v0.20.1** — "Hotfix: PostgreSQL 16 Replication Slots Compatibility":
  - **Replication Slots Version Gate**: fixed `s.invalidated` column reference for PostgreSQL 16 by bumping the `invalidated` column gate to PG 17+ (`replication_slots_post_170000.sql`). PostgreSQL 16 now seamlessly falls back to the clean query with `NULL::text AS invalidated`.
- **v0.20.0** — "Logical & Physical Replication Deep-Dive: Dedicated Publications, Enriched Subscriptions & Slots":
  - **Dedicated Publications Panel**: catalog discovery for logical publications (`pg_publication`, `pg_publication_tables`) with published tables list, schema scoping, and operation flags (`INSERT`, `UPDATE`, `DELETE`, `TRUNCATE`) in TUI and Web Lens.
  - **Enriched Subscriptions**: subscriber telemetry (`pg_subscription`, `pg_stat_subscription`, PG 15+ version gate) with sanitized connection parameters, streaming/binary modes, two-phase commit, worker count, and syncing tables.
  - **Enriched Replication Slots**: expanded slot diagnostics (`pg_replication_slots`, PG 16+ version gate) with plugin, database, client app/address, restart/flush LSN, consumer lag, two-phase, and invalidated/conflicting flags.
  - **Replication Slot Detail Modal**: interactive floating detail dialog on `Enter` in TUI Replication Lens displaying complete connection, LSN positions, and WAL retention metrics.
  - **Responsive Layout**: multi-tier responsive slot columns in TUI and clean distinct sections in Web Lens.
- **v0.19.0** — "Observability Expansion: Sequences, SLRU, Standby Conflicts & Table Storage":
  - **Sequence Exhaustion Alerts**: proactive monitoring of `pg_sequences` in core, TUI Schema Lens sub-view (`S`), and Web Lens panel with exhaustion percentage and capacity calculation.
  - **SLRU Cache Monitoring**: Simple LRU cache monitoring (`pg_stat_slru`, PG 13+) on TUI Macro Lens and Web vitals with subtransaction thrashing warning.
  - **Standby Recovery Conflicts**: standby replica recovery conflict tracking (`pg_stat_database_conflicts`) on TUI Replication Lens and Web status row.
  - **Table to Index Jump**: cross-lens jump from Schema Lens to Index Lens filtered by table (`i`), with `Backspace`/`Esc` returning back.
  - **TOAST & Cache Breakdown**: separated heap and TOAST storage sizes and cache hit ratios for tables in TUI and Web detail views.
  - **Replication Lens Renaming**: Tab 4 renamed to "4 Replication Lens" across TUI and Web.
- **v0.18.1** — "Records Lens & Streaming Compression":
  - **Records Lens**: dedicated recording archive manager in TUI (Tab 9 `9 Records Lens`) and Web Lens (Tab 8 `Records`) with search filtering (`/`), file metadata, in-app replay (`Enter`), path copy (`y`), and interactive/token-gated deletion (`x`).
  - **Streaming Compression**: pure-Rust streaming gzip compression (`.jsonl.gz`) via `flate2` (`--record-compress`), transparently detected and read by `RecordingReader::load`.
  - **Auto-Split & Retention Policies**: `--record-max-mb` file size auto-rotation (100 MB default), `--record-retention-days` (30 days default), `--record-max-total-mb` storage limit (1000 MB default), and FIFO auto-pruning.
  - **Visual Replay Scrubber**: interactive timeline track (`[████░░░] 42%`) in TUI replay mode with `Home`/`End` frame jumping, loop toggle (`l`/`L`), and speed adjustment (`[`/`]`).
- **v0.18.0** — "Incident Recording & Flight Recorder":
  - **Live Incident Recording**: continuous streaming capture of `DbSnapshot` frames to `.jsonl` (`~/.local/state/pg_lens/recordings/rec-<target>-<ts>.jsonl`) toggled via `Shift+R` (`R`) or `Ctrl+R` in TUI and `● REC` button in Web Lens, with live pulse timer and frame count; file path copied to clipboard via OSC 52 on stop.
  - **Snapshot Bookmark Export**: point-in-time pretty JSON bookmark (`~/.local/state/pg_lens/exports/snapshot-<target>-<ts>.json`) via `E` key (TUI) and `Export` button (Web Lens), with file path copied to clipboard via OSC 52.
  - **Offline Interactive Replay**: `pg_lens replay <file.jsonl> [--speed <f64>] [--loop-playback]` and `pg_lens view <file.json|file.jsonl>` for offline incident diagnosis without requiring a PostgreSQL connection (`Space` to play/pause, `←`/`→` to step frames, `[`/`]` to adjust speed 0.25x–16.0x, `E` to bookmark).
  - **Bloat & Schema Refresh Rebind**: schema and bloat recollect rebound to `Shift+B` (`B`) across all lenses, statusbar, and help overlay, cleanly freeing `R` for Flight Recorder.
- **v0.17.2** — "Column Colors & Duration Severity": replaced whole-row tinting in
  the Micro Lens activity table with a dedicated column-specific color system
  inspired by pg_activity (semantic colors per column); and restricted time-based
  coloring strictly to the Duration column (>30s red bold, >10s yellow bold, <=10s
  calm green for active; idle stays calm dark gray).
- **v0.17.1** — "Progress Lens": dedicated Progress Lens (`8 Progress Lens` in TUI,
  `7 Progress` in Web Lens) for real-time monitoring of all in-flight PostgreSQL maintenance
  and DDL operations (`pg_stat_progress_create_index`, `pg_stat_progress_vacuum`,
  `pg_stat_progress_cluster`, and `pg_stat_progress_analyze`) with visual progress gauges,
  step counters, operational details, selection, and admin cancellation/termination; and
  renamed Tab 3 to "3 Blocks & Locks Lens" for naming consistency across TUI and Web.
- **v0.17.0** — "Blocks & Locks Lens": dedicated Blocks tab (wait-for tree +
  active locks) in position 3 between Micro Lens and Replication; in-flight DDL &
  maintenance progress (`pg_stat_progress_*`); and SSL/TLS connection security
  indicators (`pg_stat_ssl`).
- **v0.16.0** — "First impression": curl | sh installer (`scripts/install.sh`);
  whole-row activity colors (pg_activity-style); copy-to-clipboard (`y`, OSC 52);
  `pg_stat_io` profile (PG 16+); `pg_stat_wal` generation rate (PG 14+); and shell
  completions subcommand.

- **v0.15.0** — "Schema Lens completo": honest `N of M tables` counts with
  a configurable, perf-bounded `--schema-table-limit` (bug fix for a
  silent `LIMIT 200`); on-demand table structure detail on `Enter`
  (columns, constraints, referencing FKs, index definitions, fetched via
  a new request/response channel, never on the poll cadence); native
  partitioned-table collapsing into an aggregated parent row (`p` reveals
  leaves), including a fix for PG16 double-counting an all-zero parent
  stats row; and an `x` cross-lens jump from a selected table into a
  Query-Lens filter for statements mentioning it, plus a new per-table
  lock indicator — all in both TUI and Web Lens.
- **v0.14.0** — "See the trend, not just the moment": widened the
  persisted `SnapshotHistory` with connections/cache-hit%/lock-pressure%/
  oldest-XID-age and added vitals trend arrows (↑/↓/→ vs ~5 min ago) to the
  Macro Lens Connections/Cache hit/Lock table cards, TUI + web (web adds a
  tooltip delta); a web-only history time-scrubber (hover for a live
  readout, click to pin keyed by timestamp, ←/→ step, Esc/✕ unpin); a
  Schema Lens `Δ1h` table-size-growth column from a bounded oid-keyed
  per-table ring, with a new "growth" `s` sort mode; a Query Lens `Temp`
  column plus a full temp-spill/I/O detail breakdown (temp read/written,
  shared blocks dirtied/written, block I/O timing, WAL bytes) across three
  `pg_stat_statements` extension-version gates, with a new "temp" `s` sort
  mode; and an interactive TTY service picker for `pg_lens serve` on an
  ambiguous `services.toml` (non-TTY keeps the v0.13 fail-loud).
- **v0.13.0** — consolidated release of two batches (versions skip
  0.12.0 — one tag, one pipeline): "Navigation & filters" — direct tab
  jump (`1`–`6`), `Shift+Tab` backward cycle, `Backspace` last-tab
  toggle, fast scroll (`Home`/`g`, `End`/`G`, `PageUp`/`PageDown`) on
  every selectable table, and a `/` filter extended to the Schema Lens
  (Tables view) and Query Lens with per-lens state and a `\` clear —
  all in both TUI and Web Lens; and "Web catch-up & redesign" — a
  modern dashboard redesign (sidenav, topbar, light/dark theme toggle,
  inline SVG icons), a token-gated database switcher
  (`POST /api/db/switch`, works even under `--read-only`), web keyboard
  navigation (`1`–`5`, `/`, `Esc`), and `pg_lens serve` fail-loud on an
  ambiguous multi-service config instead of silently connecting to
  localhost.
- **v0.11.0** — "Incident precursors & connection visibility": idle
  connection / connection-age census (`I` toggle in the Micro Lens, oldest
  idle sessions ranked with age tiers — diagnoses pool-exhaustion incidents
  the activity table normally hides); a lock-table pressure gauge (third
  Macro Lens vitals gauge, warns ahead of "out of shared memory, increase
  max_locks_per_transaction"); an `INVALID`/not-ready index flag in the
  Index Lens for indexes left behind by a failed `CREATE INDEX
  CONCURRENTLY`; and a `psql` shell launch (`!`, TUI-only) on the same
  connection, password only via `PGPASSWORD` env (never argv), with a
  read-only-transaction default under `--read-only` — all in both TUI and
  Web Lens except the TUI-only psql shell.
- **v0.10.0** — read-only mode (`--read-only` / `PG_LENS_READ_ONLY` /
  `read_only = true` in config.toml) hard-disables `c`/`K` with a real
  server-side gate in both the TUI (refused before the confirm modal opens,
  permanent yellow `RO` header marker) and the Web Lens (`403` on
  `/api/admin/*` even with a valid `PG_LENS_AUTH_TOKEN`); remote connection
  config (`--config-url` / `PG_LENS_CONFIG_URL` / `remote_config` in
  config.toml) loads a shared `services.toml` from a
  `github:owner/repo/path@ref` shorthand or any https(s) URL, with a
  token-never-in-a-file resolution chain, a local cache + offline fallback,
  and remote-wins precedence — all in both TUI and Web Lens where applicable.
- **v0.9.0** — "Problem transactions": idle-in-transaction / transaction-age
  hunter (Micro Lens column + oldest-open-xact headline, yellow/red tiers),
  blocking-chain / lock-wait graph in the Micro Lens detail panel (root
  blocker highlighted, deadlock-cycle detection), orphaned prepared-transaction
  (2PC) watch inside the Vacuum sub-view, a keyboard help overlay (`?`), and
  the `docs/connection-user.md` least-privilege guide — all in both TUI and
  Web Lens.
- **v0.8.0** — "Room to breathe": Index Lens and Replication Lens as their
  own tabs (six tabs total; all replication slots scrollable), database
  selector (`d` reconnects the poller to any database on the cluster), full
  waits panel (`w`) and a Vacuum sub-view (`v`) — all in both TUI and Web
  Lens.
- **v0.7.1** — `serve` Ctrl+C no longer hangs with SSE clients attached
  (`b2b9856`), replication slots no longer pushed out by many active WAL
  senders (`b2b9856`), TUI double-Esc quit barrier (`14b3e78`).
- **v0.7.0** — "What should I go fix": top waits panel (`4e94fe0`), vacuum
  health & XID wraparound (`9ed90dd`), replication slots view (`7addcd5`),
  index advisor for unused/duplicate/prefix-redundant indexes (`ad3c2d8`),
  checkpointer/bgwriter panel version-gated at PG17 (`3eef2f6`) — all five
  in both TUI and Web Lens.
- **v0.6.1** — cancel in-flight queries on shutdown (CancelRequest on quit).
- **v0.6.0** — config.toml defaults; persistent history (JSONL per target,
  1h ring); Micro Lens activity filter (`/` + web search box); Web Lens
  parity (pause, schema refresh, token-gated admin).
- **v0.5.x** — pooler-safe polling (per-tick read-only transactions +
  `SET LOCAL statement_timeout`), global CLI connection flags, poll
  resilience on restricted/managed servers, cask quarantine self-clear,
  crates.io publishing.
- **v0.5.0** — Query Lens (pg_stat_statements); replication/WAL panel;
  spacebar pause; admin actions (`c`/`K`); web SQL highlighting.
- **v0.3–0.4** — Schema Lens (stats + ioguix bloat); service picker; TUI
  polish; demo assets.
- **v0.1–0.2** — MVP (Macro/Micro Lens, real data layer, resilience); Web
  Lens (axum + SSE + embedded frontend); connections (env vars,
  services.toml + password_cmd); full distribution (brew/deb/rpm/binaries).
