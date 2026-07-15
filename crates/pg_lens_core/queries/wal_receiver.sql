-- WAL receiver status of this server (standby side).
--
-- Adapted from dalibo/pg_activity's get_wal_receiver queries. Zero rows on a
-- primary (no receiver); at most one row on a standby. `sender_host` /
-- `sender_port` identify the upstream. Two lag measures:
--   * bytes: received-but-not-yet-replayed WAL on this standby.
--   * secs:  wall-clock age of the last replayed transaction's commit.
-- Both LSN/time functions are valid on a primary too (they return 0/NULL),
-- so no recovery guard is needed here.
SELECT
      status,
      sender_host,
      sender_port,
      pg_wal_lsn_diff(pg_last_wal_receive_lsn(), pg_last_wal_replay_lsn())::int8
          AS replay_lag_bytes,
      EXTRACT(epoch FROM (now() - pg_last_xact_replay_timestamp()))::float8
          AS replay_lag_secs
 FROM pg_stat_wal_receiver;
