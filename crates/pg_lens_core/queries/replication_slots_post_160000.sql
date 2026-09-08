-- Replication slots (pg_replication_slots, PG 16+).
--
-- Includes invalidated column (added in PG 16: wal_removed / max_slot_wal_keep_size).
SELECT
      s.slot_name::text AS slot_name,
      s.plugin::text AS plugin,
      s.slot_type::text AS slot_type,
      d.datname::text AS database,
      s.temporary,
      s.active,
      s.active_pid::int4 AS active_pid,
      a.application_name::text AS application_name,
      a.client_addr::text AS client_addr,
      s.restart_lsn::text AS restart_lsn,
      s.confirmed_flush_lsn::text AS confirmed_flush_lsn,
      CASE WHEN pg_is_in_recovery() OR s.restart_lsn IS NULL THEN NULL
           ELSE pg_wal_lsn_diff(pg_current_wal_lsn(), s.restart_lsn)::int8
      END AS retained_wal_bytes,
      CASE WHEN pg_is_in_recovery() OR s.confirmed_flush_lsn IS NULL THEN NULL
           ELSE pg_wal_lsn_diff(pg_current_wal_lsn(), s.confirmed_flush_lsn)::int8
      END AS consumer_lag_bytes,
      s.wal_status::text AS wal_status,
      s.safe_wal_size::int8 AS safe_wal_size,
      CASE WHEN s.xmin IS NOT NULL THEN age(s.xmin)::int8 ELSE NULL END AS xmin_age,
      CASE WHEN s.catalog_xmin IS NOT NULL THEN age(s.catalog_xmin)::int8 ELSE NULL END AS catalog_xmin_age,
      s.two_phase,
      s.conflicting,
      s.invalidated::text AS invalidated
 FROM pg_replication_slots s
 LEFT JOIN pg_database d ON d.oid = s.datoid
 LEFT JOIN pg_stat_activity a ON a.pid = s.active_pid
ORDER BY retained_wal_bytes DESC NULLS LAST;
