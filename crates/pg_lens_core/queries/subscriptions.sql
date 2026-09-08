-- Logical replication subscriptions for the current database (PG 13-14).
--
-- Lists subscriptions defined on this database, connected worker status,
-- received/applied LSN progress, and table sync state (from pg_subscription_rel).
SELECT
      s.subname::text AS subname,
      r.rolname::text AS owner,
      s.subenabled,
      s.subslotname::text AS subslotname,
      s.subpublications,
      s.subsynccommit::text AS sync_commit,
      substring(s.subconninfo from 'host=([^ ]+)') AS publisher_host,
      substring(s.subconninfo from 'port=([^ ]+)') AS publisher_port,
      substring(s.subconninfo from 'dbname=([^ ]+)') AS publisher_dbname,
      NULL::text AS streaming_mode,
      NULL::bool AS binary_mode,
      NULL::bool AS two_phase,
      stat.pid::int4 AS pid,
      stat.received_lsn::text AS received_lsn,
      EXTRACT(epoch FROM stat.last_msg_send_time)::float8 AS last_msg_send_secs,
      EXTRACT(epoch FROM stat.last_msg_receipt_time)::float8 AS last_msg_receipt_secs,
      stat.latest_end_lsn::text AS latest_end_lsn,
      EXTRACT(epoch FROM stat.latest_end_time)::float8 AS latest_end_secs,
      COALESCE(sync_summary.sync_tables, 0)::int8 AS sync_tables,
      COALESCE(sync_summary.ready_tables, 0)::int8 AS ready_tables,
      COALESCE(sync_summary.total_tables, 0)::int8 AS total_tables,
      sync_summary.syncing_table_names,
      NULL::int8 AS apply_error_count,
      NULL::int8 AS sync_error_count
 FROM pg_subscription s
 JOIN pg_roles r ON r.oid = s.subowner
 LEFT JOIN pg_stat_subscription stat ON stat.subid = s.oid AND stat.relid IS NULL
 LEFT JOIN (
      SELECT
          sr.srsubid,
          count(*)::int8 AS total_tables,
          count(CASE WHEN sr.srsubstate = 'r' THEN 1 END)::int8 AS ready_tables,
          count(CASE WHEN sr.srsubstate IN ('i', 'd', 's') THEN 1 END)::int8 AS sync_tables,
          string_agg(CASE WHEN sr.srsubstate IN ('i', 'd', 's') THEN c.relname || ' (' || CASE sr.srsubstate WHEN 'i' THEN 'init' WHEN 'd' THEN 'copy' WHEN 's' THEN 'sync' END || ')' END, ', ' ORDER BY c.relname) AS syncing_table_names
       FROM pg_subscription_rel sr
       LEFT JOIN pg_class c ON c.oid = sr.srrelid
      GROUP BY sr.srsubid
 ) sync_summary ON sync_summary.srsubid = s.oid
WHERE s.subdbid = (SELECT oid FROM pg_database WHERE datname = current_database())
ORDER BY s.subname;
