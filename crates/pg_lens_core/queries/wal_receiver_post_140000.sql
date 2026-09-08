-- WAL receiver status of this server (standby side, PG 14+).
--
-- Includes pg_get_wal_replay_pause_state() which distinguishes between
-- 'not paused', 'pause requested', and 'paused'.
SELECT
      status,
      sender_host,
      sender_port,
      pg_wal_lsn_diff(pg_last_wal_receive_lsn(), pg_last_wal_replay_lsn())::int8
          AS replay_lag_bytes,
      EXTRACT(epoch FROM (now() - pg_last_xact_replay_timestamp()))::float8
          AS replay_lag_secs,
      pg_is_wal_replay_paused() AS is_paused,
      pg_get_wal_replay_pause_state() AS pause_state
 FROM pg_stat_wal_receiver;

