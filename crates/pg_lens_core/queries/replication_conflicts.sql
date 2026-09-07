-- Database recovery conflicts for standby replicas (v0.19): pg_stat_database_conflicts.
-- Runs on the FAST tick (2s) — cheap single-row view.
SELECT
    datid::int8 AS datid,
    datname::text AS datname,
    confl_tablespace::int8 AS confl_tablespace,
    confl_lock::int8 AS confl_lock,
    confl_snapshot::int8 AS confl_snapshot,
    confl_bufferpin::int8 AS confl_bufferpin,
    confl_deadlock::int8 AS confl_deadlock
FROM pg_stat_database_conflicts
WHERE datname = current_database();
