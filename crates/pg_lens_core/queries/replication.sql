-- Streaming replicas connected to this server (primary side).
--
-- Adapted from dalibo/pg_activity's get_wal_senders queries; simplified to
-- the columns the Macro Lens replication panel needs. One row per connected
-- standby (empty on a primary with no replicas, and — normally — empty when
-- run on a standby, which is why the LSN diff below is still guarded).
--
-- `pg_current_wal_lsn()` errors during recovery, but pg_stat_replication has
-- no rows on a (non-cascading) standby, so the SELECT list is never evaluated
-- there; a cascading standby is protected by the pg_is_in_recovery() guard,
-- which short-circuits the CASE before the LSN function runs.
--
-- Visibility: the lag/LSN columns of pg_stat_replication require the
-- pg_monitor role (or superuser); a non-privileged user sees zero rows.
SELECT
      application_name,
      coalesce(client_addr::text, 'local') AS client,
      state,
      sync_state,
      CASE WHEN pg_is_in_recovery() THEN NULL
           ELSE pg_wal_lsn_diff(pg_current_wal_lsn(), replay_lsn)::int8
      END AS replay_lag_bytes,
      EXTRACT(epoch FROM replay_lag)::float8 AS replay_lag_secs
 FROM pg_stat_replication
ORDER BY replay_lag_bytes DESC NULLS LAST;
