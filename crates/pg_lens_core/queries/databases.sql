-- Databases available on this cluster (U2, the in-session database picker,
-- `d` in the TUI). Own query, not adapted from pg_activity/dalibo. Runs on
-- the FAST tick, best-effort: `pg_database` itself is readable by everyone,
-- but a restricted role can still fail here in principle, so this rides the
-- same "absent this tick, never a poll fault" contract as replication/
-- vacuum progress.
--
-- `datallowconn` excludes template0 (never connectable); `NOT datistemplate`
-- additionally hides template1 and any other template database — neither is
-- a database an operator would ever want to switch pg_lens into.
--
-- Size is best-effort PER ROW: `pg_database_size()` raises
-- "permission denied" for a database the connected role lacks CONNECT
-- privilege on (any database other than its own, commonly, under a
-- restricted monitoring role) — guarding it with `has_database_privilege`
-- turns that into a calm NULL (rendered as "--") instead of failing the
-- whole query and hiding every other database's name too.
SELECT
      datname::text AS datname,
      CASE WHEN has_database_privilege(datname, 'CONNECT')
           THEN pg_database_size(datname)::int8
           ELSE NULL
      END AS size_bytes
 FROM pg_catalog.pg_database
WHERE datallowconn
  AND NOT datistemplate
ORDER BY datname;
