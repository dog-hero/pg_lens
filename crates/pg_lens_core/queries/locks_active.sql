-- Active locks in current database (for Blocks Lens, v0.17).
--
-- Joins pg_locks with pg_class, pg_namespace and pg_stat_activity.
-- Scoped to the connected database and shared relations (database = 0).
-- Prioritizes ungranted (waiting) locks first, ordered by duration.
SELECT
      l.pid AS pid,
      l.locktype AS locktype,
      coalesce(c.relname::text, '') AS relation,
      coalesce(n.nspname::text, '') AS schema,
      l.mode AS mode,
      l.granted AS granted,
      coalesce(l.fastpath, false) AS fastpath,
      coalesce(EXTRACT(epoch FROM (NOW() - coalesce(a.query_start, a.xact_start, now())))::float8, 0.0::float8) AS duration_secs,
      coalesce(a.usename::text, '') AS usename,
      coalesce(a.application_name::text, '') AS application_name,
      coalesce(a.query::text, '') AS query
 FROM pg_catalog.pg_locks l
 LEFT JOIN pg_catalog.pg_class c ON c.oid = l.relation
 LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
 LEFT JOIN pg_catalog.pg_stat_activity a ON a.pid = l.pid
WHERE (l.database = (SELECT oid FROM pg_catalog.pg_database WHERE datname = current_database()) OR l.database = 0)
  AND l.pid <> pg_catalog.pg_backend_pid()
ORDER BY l.granted ASC, duration_secs DESC
LIMIT 200;

