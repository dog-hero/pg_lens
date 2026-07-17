# pg_lens — Roadmap

Execution order for [PRD.md](PRD.md). Sourced from the `feature-discovery`
agent's 2026-07-15 research (comparables: pg_activity, pghero, pganalyze,
pgcenter, pg_top) plus the owner's priorities. Every item follows the PRD's
Definition of Done. Check items off as they ship; move them to the Shipped
section on release.

---

## v0.9 — "Problem transactions" (in progress — owner-selected 2026-07-16)

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

## v0.10 candidates (mapped 2026-07-16 — owner priorities)

- [ ] **Remote connection config** — load connection definitions
  (`services.toml`-shaped) from a remote source, including a **private GitHub
  repo**, so a team shares one curated target list instead of copying files.
  Design notes / open questions: auth (GitHub token via env/keychain, reuse
  the `password_cmd` secret pattern — never a token in the file); fetch on
  startup with a local cache + offline fallback; a `remote_config` URL/ref in
  `config.toml` and/or a `--config-url` flag; refuse to persist secrets;
  precedence vs. local `services.toml`. Read-only fetch — pg_lens never writes
  back to the repo.
- [ ] **Read-only mode (no actions)** — a mode that hard-disables every
  admin/mutating action (`c` cancel, `K` terminate, schema/bloat refresh
  writes if any) and the write transaction path, for shared/audited
  deployments and least-privilege roles. Surfaced in the header; enforced in
  `update()` and the web admin endpoints (not just hidden in the UI); settable
  via `config.toml`, a `--read-only` flag, and ideally auto-detected when the
  role lacks the privilege. Pairs with the v0.9 least-privilege docs.

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
