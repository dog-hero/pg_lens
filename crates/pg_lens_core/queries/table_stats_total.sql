-- Schema Lens: TRUE table count (v0.15 honest-counts fix).
--
-- `table_stats_post_130000.sql`'s `ranked` CTE caps its own output at the
-- configured `schema_table_limit` (default 200) — table 201+ (by the
-- relpages-ranked cut) never appears in that result set, so the row count
-- of THAT query can never be trusted as "how many tables does this
-- database actually have". This is a separate, uncapped scalar query run
-- in the SAME read-only transaction as table_stats, so the footer's
-- "N of M tables" is honest even when zero rows survive the ranked query's
-- LIMIT (e.g. a freshly created, still-empty database) — a case a
-- `count(*) OVER ()` window function folded into table_stats itself would
-- get wrong, since a window function's count is taken over the LIMITed
-- result set, not the pre-LIMIT one, and a CROSS JOIN against a zero-row
-- ranked CTE would silently drop the total along with the rows.
--
-- Cheap: `pg_stat_user_tables` is an in-memory catalog/stats view, and
-- `count(*)` over it touches no table data or indexes — safe to run every
-- slow tick alongside table_stats.
--
-- v0.15: excludes partitioned-table PARENTS (`relkind = 'p'`) — verified
-- live against PG16 that `pg_stat_user_tables` carries an all-zero row for
-- the parent itself (see `table_stats_post_130000.sql`'s header), which
-- `table_stats` also excludes; counting it here but not there would make
-- "N of M tables" report a total one higher than could ever be reached
-- (the parent is represented by its own AGGREGATED row from
-- `partition_parents.sql` instead, not a `table_stats` row).
SELECT count(*)::int8 AS tables_total
  FROM pg_stat_user_tables AS s
  JOIN pg_class AS c ON c.oid = s.relid
 WHERE c.relkind <> 'p';
