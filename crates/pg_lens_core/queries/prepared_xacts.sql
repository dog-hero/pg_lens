-- Orphaned two-phase-commit (2PC) watch (v0.9), `pg_prepared_xacts`. A
-- prepared transaction left un-COMMITted/un-ROLLBACKed holds its locks and
-- pins the wraparound horizon indefinitely — the classic silent incident:
-- nothing in pg_stat_activity shows it (the backend that PREPARE TRANSACTION
-- ran already disconnected), so it blocks vacuum forever without a visible
-- session to blame. Own query, not adapted from pg_activity/dalibo.
--
-- `pg_prepared_xacts` is a stable system view across the whole 13+ range —
-- no post_NNNNNN variant needed. Collected on the FAST tick, best-effort:
-- treated exactly like replication/vacuum-progress (absent this tick on any
-- failure, never a poll fault), even though the view itself is world-
-- readable — see `poller::collect_prepared_xacts`.
--
-- owner/database are `name`-typed columns — cast ::text for a clean String
-- map (same reasoning as `statements.sql`'s rolname cast). EXTRACT(epoch...)
-- is `numeric` on PG >= 14 but `float8` on 13, so `age_seconds` is always
-- cast ::float8 (the well-known type trap).
SELECT
      gid::text AS gid,
      owner::text AS owner,
      database::text AS database,
      EXTRACT(epoch FROM (now() - prepared))::float8 AS age_seconds
 FROM pg_catalog.pg_prepared_xacts
ORDER BY prepared;
