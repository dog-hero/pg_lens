-- Replication slots (pg_replication_slots), F2.5 — both physical and
-- logical, present on primaries AND standbys alike (unlike
-- pg_stat_replication / pg_stat_wal_receiver, which are role-specific), so
-- this query runs on every fast tick regardless of pg_is_in_recovery().
--
-- The point of the feature: an INACTIVE slot that is still retaining WAL is
-- the classic full-disk incident — nothing is consuming it, so WAL piles up
-- in pg_wal until the disk fills. `restart_lsn` is the oldest WAL a slot
-- still needs; the retained-bytes diff is guarded exactly like
-- queries/replication.sql (pg_current_wal_lsn() errors during recovery) and
-- additionally against a NULL restart_lsn (a logical slot that has never
-- been used yet has none).
--
-- wal_status / safe_wal_size are PG 13+ (pg_lens's version floor), so one
-- file serves the whole supported range — no post_NNNNNN variant needed.
--
-- Visibility: pg_replication_slots is readable by any role (unlike the lag
-- columns of pg_stat_replication), so this degrades gracefully even under a
-- restricted monitoring role; only pg_current_wal_lsn() during recovery can
-- raise, and that path is short-circuited by the CASE guard.
SELECT
      slot_name::text AS slot_name,
      slot_type::text AS slot_type,
      active,
      CASE WHEN pg_is_in_recovery() OR restart_lsn IS NULL THEN NULL
           ELSE pg_wal_lsn_diff(pg_current_wal_lsn(), restart_lsn)::int8
      END AS retained_wal_bytes,
      wal_status::text AS wal_status,
      safe_wal_size::int8 AS safe_wal_size
 FROM pg_replication_slots
ORDER BY retained_wal_bytes DESC NULLS LAST;
