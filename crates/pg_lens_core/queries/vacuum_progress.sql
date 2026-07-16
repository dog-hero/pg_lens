-- In-flight VACUUM progress (F2), `pg_stat_progress_vacuum` — usually empty
-- (no vacuum running right now), which the UI renders calmly rather than as
-- an empty table. Collected on the FAST tick, best-effort: a restricted
-- role or a server that hides the view degrades to "no panel this tick",
-- exactly like replication — this must never fail the poll.
--
-- relid is resolved to a name via pg_class (LEFT JOIN: a relation dropped
-- mid-scan since the view was last refreshed would otherwise vanish the
-- whole row instead of just its name).
SELECT
      v.pid,
      coalesce(c.relname::text, '?') AS relation,
      v.phase,
      coalesce(v.heap_blks_total, 0)::int8 AS heap_blks_total,
      coalesce(v.heap_blks_scanned, 0)::int8 AS heap_blks_scanned
 FROM pg_stat_progress_vacuum v
 LEFT JOIN pg_class c ON c.oid = v.relid;
