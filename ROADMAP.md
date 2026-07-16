# pg_lens — Roadmap

Execution order for [PRD.md](PRD.md). Sourced from the `feature-discovery`
agent's 2026-07-15 research (comparables: pg_activity, pghero, pganalyze,
pgcenter, pg_top) plus the owner's priorities. Every item follows the PRD's
Definition of Done. Check items off as they ship; move them to the Shipped
section on release.

---

## v0.8 — "Room to breathe" (owner UX feedback 2026-07-16, in progress)

Field feedback after using v0.7 on a real cluster: the new data is there but
buried — the index advisor hides behind a toggle, slots share a cramped
panel, waits/vacuum are easy to miss, and everything is locked to the DSN's
database on clusters with many databases.

- [ ] **U1 — Tab restructure: Index Lens + Replication Lens** *(M)* — six
  tabs: Macro │ Micro │ Replication │ Schema │ Indexes │ Queries. The index
  advisor gets its own lens (own selection/detail/statusbar hints; the `i`
  toggle inside Schema goes away). The Replication Lens shows
  senders/receiver at the top and ALL slots as a scrollable (j/k) table —
  no more "+N more" clipping; the Macro panel stays as the compact summary.
- [ ] **U2 — Database selector** *(M/L)* — `d` opens a database picker
  (`pg_database` list, sizes, current highlighted). Selecting reconnects the
  poller to the same host/creds with the chosen `dbname` (PostgreSQL cannot
  switch databases in-session), reusing the startup-picker UI and the
  poller's existing reconnect machinery. Per-database lenses
  (Schema/Indexes/Queries) follow automatically; header shows the database.
- [ ] **U3 — Waits & vacuum visibility** *(S/M)* — Micro: `w` toggles a full
  waits panel (complete ranked list with counts/percent, not just the
  one-line strip). Schema: the vacuum/wraparound block becomes a proper
  sub-view with room (worst tables list + progress), not a footer squeezed
  under the tables.

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
