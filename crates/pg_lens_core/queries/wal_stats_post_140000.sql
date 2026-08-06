-- WAL generation rate (v0.16, PG 14+ ONLY): `pg_stat_wal`, a single-row
-- cluster-wide cumulative view of WAL activity since the last stats reset
-- (`pg_stat_reset_shared('wal')`) or server start. The view does not exist
-- below PG 14, which is why this file has no `post_130000` sibling and
-- `queries::for_version` selects `None` there instead (mirroring
-- `io_post_160000.sql`'s PG-16 floor).
--
-- `wal_bytes` is `numeric` (it can outgrow `bigint` over a very long
-- uptime, unlike the plain counters) -- cast to `::int8` here, the exact
-- same convention `statements_ext_1_9.sql` already established for
-- pg_stat_statements' own per-statement `wal_bytes` column.
--
-- `wal_write_time`/`wal_sync_time` are `double precision` already (no cast
-- trap there), but read back as 0 -- not NULL -- when `track_wal_io_timing`
-- is off, indistinguishable from "no time spent". `track_wal_io_timing_on`
-- rides along as a per-row scalar so the Rust parser can null both out in
-- that case instead of shipping a misleading zero, same convention as
-- `io_post_160000.sql`'s `track_io_timing_on`.
SELECT
    wal_records::int8 AS wal_records,
    wal_fpi::int8 AS wal_fpi,
    wal_bytes::int8 AS wal_bytes,
    wal_buffers_full::int8 AS wal_buffers_full,
    wal_write_time::float8 AS wal_write_time_ms,
    wal_sync_time::float8 AS wal_sync_time_ms,
    (current_setting('track_wal_io_timing') = 'on') AS track_wal_io_timing_on
FROM pg_catalog.pg_stat_wal;
