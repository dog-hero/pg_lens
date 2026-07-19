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

## v0.15 — "Charts dashboard" (active)

Owner picks from the v0.8+ candidate list (pg_stat_io, DDL progress, snapshot
export) plus the 2026-07-19 discovery pass themed "a really good charts
dashboard". Key finding: the gap versus pganalyze/pgwatch2-style dashboards is
almost entirely wiring, not new SQL — the fast tick already computes nearly
every number those tools chart (connections by state, checkpoint/bgwriter
rates, temp bytes, deadlocks, replication lag) and silently discards it
instead of recording it in `HistoryPoint`. Dependency order: item 2 can ship
immediately (frontend-only); item 1 unlocks items 3–5; item 6 (the Charts tab)
consolidates everything; the owner-picked items 7–9 are independent.

- [ ] **Widen `HistoryPoint` v2 — the chart unlock** — add per-tick scalars
  the poller already holds (`#[serde(default)]`, old JSONL keeps loading):
  `idle` / `idle_in_transaction` / `waiting` counts (from `ServerVitals`),
  `checkpoints_per_min_timed` / `buffers_checkpoint_per_sec` /
  `buffers_clean_per_sec` (from `CheckpointerStats`), `temp_bytes_delta` /
  `deadlocks_delta` (new entries in the poller's `DeltaState`, mirroring the
  existing `xact_total` delta), `longest_xact_age_secs` + `blocked_count`
  (one-line aggregations over activity rows), `max_replica_lag_bytes`
  (max over WAL senders, `None` on standby/no replicas). Mirror in
  `types.ts` + `DbSnapshot::mock()`. Zero new SQL. Foundational. **M.**
- [ ] **Chart the trends already streaming** — `cache_hit_pct` and
  `lock_pressure_pct` have sat in `HistoryPoint` since v0.14 but are only
  used for trend arrows, never plotted; add them as uPlot series following
  the existing `tps`/`active_sessions` wiring in `chart.ts`. Frontend-only,
  independent of everything else — ship first. **S.**
- [ ] **Trend chart pack** (each an S extension once item 1 lands):
  connections-by-state stacked area (pool exhaustion as a shape change —
  the trend companion to v0.11's idle census); checkpoint/bgwriter activity
  lines (pgwatch2's flagship dashboard, from data already on the fast tick);
  temp-spill rate line + deadlock event markers (reuse the scrubber's
  canvas-hook marker technique — deadlocks are ticks, not a line);
  longest-xact-age + blocked-count lines (the trend companion to v0.9's
  problem-transactions batch); replication lag over time (hide when no
  replicas, matching the Replication Lens). **~5×S.**
- [ ] **WAL generation rate** — the one new-SQL chart: `pg_current_wal_lsn()`
  added to `server_info` (recovery-safe `CASE` guard, same pattern as
  `replication.sql`), diffed in the poller like `xact_total`, exposed as
  `wal_bytes_per_sec` in `HistoryPoint`, charted. pg_lens currently has zero
  WAL-volume visibility. **S/M.**
- [ ] **Dedicated "Charts" dashboard tab (web)** — new top-level web tab: a
  responsive small-multiples grid of every chart above, all driven by the
  `SnapshotHistory` already streamed (no new endpoint); time-range presets
  (15m/30m/1h) re-slicing the existing 1800-point ring client-side (NOT
  extended retention — the PRD's no-warehouse pillar stands). uPlot stays
  the only chart lib; verify N-instances × 1800-points redraw cost keeps the
  2s tick smooth. The headline deliverable. **L.**
- [ ] **TUI mirror — extra Macro Lens sparklines** — 2–3 more
  `ratatui::Sparkline` rows (cache-hit, checkpoint rate, WAL rate) from the
  widened history, same widget as the existing tps/sessions sparklines; cap
  there to avoid crowding — the web tab is the richer surface, consistent
  with the v0.13 precedent. **S/M.**
- [ ] **I/O profile (`pg_stat_io`, PG 16+)** — new version-gated query
  (`post_160000` SQL file + QuerySet field, following the
  `bgwriter_post_170000` gating precedent): backend_type × io_context
  reads/writes/hits/extends/fsyncs/evictions, timing columns gated on
  `track_io_timing` (`--` when off, never an error). Best-effort absent
  panel on 13–15, readable by any role. Pairs with the checkpoint/WAL
  charts. TUI + Web. **M.**
- [ ] **DDL progress in the Micro Lens detail** —
  `pg_stat_progress_create_index` / `_cluster` (also covers `REINDEX
  CONCURRENTLY`/`VACUUM FULL`) / `_analyze`, unioned by pid (near-copy of
  the `vacuum_progress.sql` best-effort pattern, `LEFT JOIN pg_class` for
  drop-safe names, no special grants): progress bar + phase for the
  selected PID in the detail panel. TUI + Web. **S/M.**
- [ ] **Snapshot export / incident bookmark** — serialize the displayed
  `DbSnapshot` (already all-`Serialize`) to
  `~/.local/state/pg_lens/exports/*.json`. TUI: export the frozen (paused)
  view — pause is a UI-side freeze, so label the export with the snapshot's
  timestamp ("exported the paused view from 14:32:06"), not "now". Web:
  settle the UX before building — either export the scrubber's pinned
  moment (partial, HistoryPoint fields only) or the live snapshot with a
  clear "live" label; decide, don't blend. **S.**

Deferred from the same pass (architecture mismatch, not value): wait-event
breakdown over time (stacked/heatmap) — `top_waits` aggregates dynamic
wait-event names per snapshot, which doesn't fit the flat-scalar
`HistoryPoint` row; needs its own discovery on historizing categorical data
without blowing up the JSONL ring.

## v0.8+ candidates (from the discovery research — re-rank before starting)

- [ ] **SSL/connection security column** — `pg_stat_ssl` marker in the
  activity table ("who's connecting in plaintext"); silent-blank when the
  view is not visible — must never break the hot 2s path.

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
