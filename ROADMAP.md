# pg_lens — Roadmap

Execution order for [PRD.md](PRD.md). Sourced from the `feature-discovery`
agent's 2026-07-15 research (comparables: pg_activity, pghero, pganalyze,
pgcenter, pg_top) plus the owner's priorities. Every item follows the PRD's
Definition of Done. Check items off as they ship; move them to the Shipped
section on release.

---

## v0.7 — "What should I go fix" (in progress)

Ranked; implement top-down so a partial release still ships the most value.

- [ ] **F1 — Top waits panel** *(S)* — aggregate `wait_event_type:wait_event`
  across the activity rows already in every snapshot into a ranked "what is
  everyone stuck on right now" list. No new SQL — computed in core from
  `DbSnapshot.activity`. Surface: Micro Lens side panel (TUI + web).
- [ ] **F2 — Vacuum health & XID wraparound** *(S/M)* — cluster-wide
  `age(datfrozenxid)` (distance to wraparound), per-table `age(relfrozenxid)`
  + dead-tuple ratio, and in-flight `pg_stat_progress_vacuum` progress.
  Surface: Schema Lens section + Macro Lens warning banner past thresholds
  (yellow >200M, red >500M xid age). PG 13+, slow cadence.
- [ ] **F3 — Index advisor (unused / redundant)** *(M, flagship)* —
  `pg_stat_user_indexes` (idx_scan, size) + duplicate detection over
  `pg_index` column lists. Presented as **signal, not verdict**: show
  stats-reset age alongside. Surface: Schema Lens sub-view (tables ⇄ indexes
  toggle), TUI + web. PG 13+, slow cadence.
- [ ] **F4 — Checkpointer / bgwriter panel** *(S)* — checkpoint frequency,
  buffers written by source, sync/write time; per-tick deltas like TPS.
  `pg_stat_bgwriter` (13–16) / `pg_stat_checkpointer` + `pg_stat_bgwriter`
  (17+, version-gated pair). Surface: Macro Lens panel.

## v0.8 candidates (from the same research — re-rank before starting)

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
