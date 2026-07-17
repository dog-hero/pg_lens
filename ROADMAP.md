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

## v0.8+ candidates (from the discovery research — re-rank before starting)

- [ ] **I/O profile** — `pg_stat_io` (PG 16+ only), backend_type × context
  reads/writes/hits with timing; best-effort absent panel on 13–15. Needs
  `track_io_timing` grace (`--` for missing timing, never an error).
- [ ] **DDL progress** — `pg_stat_progress_create_index` / `_cluster` /
  `_analyze` joined into the Micro Lens detail panel (progress bar + phase
  for the selected PID).
- [ ] **SSL/connection security column** — `pg_stat_ssl` marker in the
  activity table ("who's connecting in plaintext"); silent-blank when the
  view is not visible — must never break the hot 2s path.
- [ ] **Snapshot export / incident bookmark** — write the current (paused)
  `DbSnapshot` to `~/.local/state/pg_lens/exports/*.json` (TUI keybinding)
  and a download button (web) for postmortems.

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
