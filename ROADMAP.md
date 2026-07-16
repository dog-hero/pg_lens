# pg_lens — Roadmap

Execution order for [PRD.md](PRD.md). Sourced from the `feature-discovery`
agent's 2026-07-15 research (comparables: pg_activity, pghero, pganalyze,
pgcenter, pg_top) plus the owner's priorities. Every item follows the PRD's
Definition of Done. Check items off as they ship; move them to the Shipped
section on release.

---

## v0.7 — "What should I go fix" (implemented — release pending)

All five items are committed on main and verified against live PostgreSQL;
awaiting the qa-tester batch gate and the v0.7.0 release.

- [x] **F1 — Top waits panel** *(S, `4e94fe0`)* — ranked
  `wait_event_type:wait_event` aggregation over the snapshot's activity
  (pure in-core, no new SQL); one-line strip above the Micro Lens table,
  Lock:* red / IO:* yellow; web chips mirror it.
- [x] **F2 — Vacuum health & XID wraparound** *(S/M, `9ed90dd`)* — cluster
  `age(datfrozenxid)` headline (yellow >200M, red >500M), worst-table ages +
  dead%, in-flight `pg_stat_progress_vacuum` (fast tick, best-effort).
  Schema Lens section + Macro Lens threshold banner + web panel/chip.
- [x] **F2.5 — Replication slots view** *(S/M, owner priority, `7addcd5`)* —
  `pg_replication_slots` in the replication panel (both roles): type,
  active, retained WAL (recovery-guarded lsn diff), wal_status,
  safe_wal_size. Inactive-retaining = yellow; unreserved/lost = red.
- [x] **F3 — Index advisor (unused / redundant)** *(M, flagship, `ad3c2d8`)*
  — unused (constraint indexes never flagged), exact-duplicate and
  prefix-redundant detection in pure core code; Schema Lens `i` toggle
  (tables ⇄ indexes) with stats-reset age for freshness; web sub-tab.
- [x] **F4 — Checkpointer / bgwriter panel** *(S, `3eef2f6`)* — per-tick
  delta rates (checkpoints/min timed vs requested, buffers/s by source,
  write/sync ms), session-window pressure ratio (yellow when requested >
  timed); version-gated at PG17's catalog split.

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
