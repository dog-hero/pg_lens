-- Per-table lock indicator (v0.15): joins the already-polled lock-pressure
-- data source (`pg_locks`) down to the RELATION level so the Schema Lens can
-- show a locked table without tab-switching to the Micro Lens's blocking
-- chain. A new query rather than reusing `blocking.sql`/`lock_capacity.sql`:
-- `blocking.sql` only reports BLOCKED sessions (pg_blocking_pids), not every
-- granted lock, and `lock_capacity.sql` is a single cluster-wide scalar with
-- no relation grouping — neither carries the per-table shape this needs.
-- Own query, not adapted from pg_activity/dalibo.
--
-- `pg_locks` is world-readable (same posture as `lock_capacity.sql`), so
-- this rarely fails — but it is still collected best-effort (see
-- `poller::collect_relation_locks`) on the FAST tick: a restricted role or a
-- future catalog change must degrade to "no lock data this tick" (every
-- table's `lock_count`/`lock_waiters` folds to `None`), never a poll fault.
--
-- `count(*)` needs the well-known tokio-postgres aggregate ::int8 cast.
-- Scoped to the CONNECTED database only (`l.database = ...`), matching every
-- other per-database query in this lens (`table_stats`, `bloat_*`, ...) —
-- `pg_locks` is cluster-wide and would otherwise mix in other databases'
-- lock activity.
SELECT
      l.relation::int8 AS rel_oid,
      count(*)::int8 AS locks,
      count(*) FILTER (WHERE NOT l.granted)::int8 AS waiting
FROM pg_catalog.pg_locks l
WHERE l.relation IS NOT NULL
  AND l.database = (SELECT oid FROM pg_catalog.pg_database WHERE datname = current_database())
GROUP BY 1;
