# pg_lens — Product Requirements Document

> **A blazing-fast, modern TUI for PostgreSQL observability.**
> *"A microscopic view into your PostgreSQL performance."*

This is the product source of truth: what pg_lens is, who it serves, what it
must never compromise, and what "done" means for a feature. Execution order
lives in [ROADMAP.md](ROADMAP.md); engineering conventions and invariants live
in [CLAUDE.md](CLAUDE.md). Historical phase plans are archived under
[docs/archive/](docs/archive/).

---

## 1. Vision

One static binary that a DBA or backend engineer points at any PostgreSQL
(13+) and immediately sees what the server is doing — in the terminal over SSH
during an incident, or in a browser for the team. pg_activity's job, rebuilt
with btop-era UX, plus the "what should I go fix" layer (bloat, index waste,
vacuum debt) that normally requires pganalyze/pghero-class tooling.

## 2. Personas & primary scenarios

| Persona | Scenario | What they need in <10s |
|---|---|---|
| **On-call engineer** | Incident: latency spike, connection pileup | Who's blocked, on what, since when; kill switch (`c`/`K`); what everyone is waiting on |
| **DBA** | Routine health check / capacity review | Bloat, dead tuples, unused indexes, vacuum/wraparound debt, replication lag |
| **Backend dev** | "Is it the database?" during development | TPS/cache/session vitals, live query list, pg_stat_statements top offenders |
| **Platform team** | Shared visibility without shell access | The same data in a browser (`pg_lens serve`), read-only by default, token-gated actions |

## 3. Product pillars (non-negotiable)

1. **Fast** — dashboard visible right after connect (~0.5s); 2s default tick;
   slow collections (schema/bloat/statements) never block the fast path.
2. **Production-safe** — read-only transactions per tick, `SET LOCAL
   statement_timeout`, optional data is best-effort (a denied view degrades to
   an absent panel, never a dead poll), in-flight queries are cancelled on
   quit. pg_lens must be safe to point at a struggling production server.
3. **Zero-config first run** — `pg_lens` with libpq env vars just works;
   picker when a services file exists; config.toml for persistent defaults.
4. **One binary, every channel** — TUI + web server in a single static
   artifact; brew/deb/rpm/crates.io/binstall, all from one tag push.
5. **Works on restricted servers** — non-superuser roles outside `pg_monitor`,
   managed services (RDS/Aurora/Cloud SQL), session-pooling proxies. (The one
   documented exception: PgBouncer *transaction* pooling.)
6. **Signal, not verdicts** — advisory features (bloat, unused indexes) show
   the evidence and its freshness (stats age); the human decides.

## 4. Product surface (shipped, v0.6.x)

- **Macro Lens** — server vitals (connections/TPS/cache-hit/temp/deadlocks),
  TPS + active-sessions chart (persistent across restarts), replication/WAL
  panel (primary and standby sides).
- **Micro Lens** — per-session activity with blocked (`B`)/waiting (`W`)
  markers, live filter (`/`), SQL-highlighted detail, sort cycling,
  cancel/terminate with confirmation.
- **Schema Lens** — pg_stat_user_tables + sizes on a slow cadence, on-demand
  estimated bloat (ioguix), severity markers.
- **Query Lens** — pg_stat_statements top statements (extension ≥1.8),
  calm "unavailable" state with the enabling hint.
- **Web Lens** — the same snapshots over SSE at `pg_lens serve`; near-parity
  (pause, schema refresh, filter) with admin actions gated behind
  `PG_LENS_AUTH_TOKEN`.
- **Connections** — libpq env vars, services.toml with `password_cmd`,
  interactive picker, `config.toml` defaults.

## 5. Definition of Done (every feature)

- `cargo clippy --workspace --all-targets -- -D warnings` clean (merge gate).
- Unit tests for new logic; TUI render assertions where UI changed.
- Verified against a **real PostgreSQL** (Docker), including the restricted
  non-superuser path when the feature reads a new catalog/view.
- Optional data sources follow the best-effort pattern (absent ≠ error).
- Version-gated SQL follows the `post_NNNNNN` convention when a view's shape
  differs across PG 13–17.
- README updated (features/keybindings); ROADMAP.md item checked off.
- Web parity considered explicitly (implemented, or consciously deferred with
  a note in ROADMAP.md).

## 6. Out of scope (deliberate)

- Historical/long-term metrics warehouse (that's pganalyze/prometheus turf —
  we persist only the chart ring).
- Query rewriting / EXPLAIN automation.
- Multi-cluster fleet management UI (single target per process for now;
  multi-instance is a backlog exploration).
- Writing to the database beyond `pg_cancel_backend`/`pg_terminate_backend`.

## 7. Team model (agents)

Development is executed by specialized Claude Code agents defined in
`.claude/agents/` (the "team"):

| Agent | Role |
|---|---|
| `feature-discovery` | Product research: comparable tools, unused pg_stat_* views, UX gaps → ranked proposals |
| `lens-builder` | Implements a lens/panel end-to-end (SQL → core model → poller → TUI → web → tests) following house patterns |
| `qa-tester` | Pre-release gate: clippy/tests/grep invariants, CLI matrix, live-DB scenarios (restricted role, poolers, kill/restart) |
| `release-manager` | Version bump + path-dep pins + tag + pipeline watch + post-release verification |

The human owner sets direction and approves releases; agents execute.
