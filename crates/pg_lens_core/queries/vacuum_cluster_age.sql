-- Cluster-wide XID wraparound distance (F2): age(datfrozenxid), the "how
-- close to a forced shutdown" number pg_activity/pgpsql-tricks operators
-- watch. Collected on the SLOW schema cadence — cheap catalog read, but
-- tied to the schema collection's timer rather than a third one.
--
-- pg_database is a shared (cluster-wide) catalog readable by any
-- authenticated role — no pg_monitor/superuser privilege needed, unlike
-- pg_stat_replication's lag columns.
--
-- ORDER BY ... LIMIT 1 gets the worst database and its age in one row (the
-- same shape as `SELECT max(age(datfrozenxid))` plus the name that owns it,
-- without a second scan).
--
-- Thresholds live in the UI layer, not here: autovacuum_freeze_max_age
-- defaults to 200,000,000; PostgreSQL forces a shutdown to prevent
-- wraparound near ~2.1 billion.
SELECT
      age(datfrozenxid)::int8 AS max_age_xids,
      datname::text AS worst_database
 FROM pg_database
ORDER BY age(datfrozenxid) DESC
LIMIT 1;
