-- Schema Lens partition-parent aggregation (v0.15's partition collapsing +
-- drill-down): native partitioned tables (`relkind = 'p'`) have NO row of
-- their own in `pg_stat_user_tables` (no physical storage) — every LEAF
-- partition appears there instead (see `table_stats_post_130000.sql`'s
-- `is_partition`/`parent_oid` columns), which would otherwise flood the
-- Tables view with what should read as one logical table. This query lists
-- each partitioned PARENT of the connected database with stats aggregated
-- over its leaf partitions via `pg_partition_tree` (PG12+, safely within
-- pg_lens's PG13 floor):
--   * total/table/index bytes: summed `pg_total_relation_size`/
--     `pg_table_size`/`pg_indexes_size` of every LEAF (`pt.isleaf`), never
--     the parent itself (which has no storage of its own);
--   * n_live_tup/n_dead_tup/tuple counters: summed from
--     `pg_stat_user_tables`, joined on the leaf relids;
--   * partition_count: how many direct-and-indirect leaves this parent has
--     (sub-partitioned trees flatten to their leaves), feeding the `parts:
--     N` marker.
--
-- Cost: `pg_partition_tree` + the per-leaf `pg_*_size()` calls run ONLY for
-- the parent tables themselves (`relkind = 'p'`, typically a handful per
-- database) — never per-leaf-row the way the main `table_stats` query would
-- if leaves were not already bounded by ITS OWN `$1` cap. This query takes
-- the SAME `$1` (`schema_table_limit`) bind param as `table_stats`, ranking
-- candidate parents by their leaves' summed `relpages` (a plain in-memory
-- catalog column, no I/O) before computing the exact aggregate for the
-- survivors — the identical two-phase shape `table_stats_post_130000.sql`
-- already uses for the same reason.
--
-- Type notes: `::int8` on every sum/count (matches `table_stats`'s
-- convention); `::text` on schema/name. Excludes `pg_catalog`/
-- `information_schema`/`pg_toast` parents (there are none in practice, but
-- mirrors the defensive scoping of the rest of the Schema Lens).
WITH parents AS (
    SELECT c.oid, c.relnamespace, c.relname
      FROM pg_class AS c
     WHERE c.relkind = 'p'
       AND c.relnamespace IN (
             SELECT oid FROM pg_namespace
              WHERE nspname NOT IN ('pg_catalog', 'information_schema', 'pg_toast')
           )
),
ranked AS (
    SELECT p.oid
      FROM parents AS p
     ORDER BY (
         SELECT coalesce(sum(c2.relpages), 0)
           FROM pg_partition_tree(p.oid) AS pt
           JOIN pg_class AS c2 ON c2.oid = pt.relid
          WHERE pt.isleaf
     ) DESC
     LIMIT $1
),
leaves AS (
    SELECT r.oid AS parent_oid, pt.relid AS leaf_relid
      FROM ranked AS r
      JOIN pg_partition_tree(r.oid) AS pt ON true
     WHERE pt.isleaf
)
SELECT
      p.oid::int8 AS relid,
      n.nspname::text AS schemaname,
      p.relname::text AS relname,
      coalesce(sum(pg_total_relation_size(l.leaf_relid)), 0)::int8 AS total_bytes,
      coalesce(sum(pg_table_size(l.leaf_relid)), 0)::int8 AS table_bytes,
      coalesce(sum(pg_relation_size(l.leaf_relid)), 0)::int8 AS heap_bytes,
      coalesce(sum(pg_indexes_size(l.leaf_relid)), 0)::int8 AS index_bytes,
      coalesce(sum(s.seq_scan), 0)::int8 AS seq_scan,
      coalesce(sum(s.seq_tup_read), 0)::int8 AS seq_tup_read,
      coalesce(sum(s.n_tup_ins), 0)::int8 AS n_tup_ins,
      coalesce(sum(s.n_tup_upd), 0)::int8 AS n_tup_upd,
      coalesce(sum(s.n_tup_del), 0)::int8 AS n_tup_del,
      coalesce(sum(s.n_tup_hot_upd), 0)::int8 AS n_tup_hot_upd,
      coalesce(sum(s.n_live_tup), 0)::int8 AS n_live_tup,
      coalesce(sum(s.n_dead_tup), 0)::int8 AS n_dead_tup,
      count(l.leaf_relid)::int8 AS partition_count,
      coalesce(sum(io.heap_blks_read), 0)::int8 AS heap_blks_read,
      coalesce(sum(io.heap_blks_hit), 0)::int8 AS heap_blks_hit,
      coalesce(sum(io.idx_blks_read), 0)::int8 AS idx_blks_read,
      coalesce(sum(io.idx_blks_hit), 0)::int8 AS idx_blks_hit,
      coalesce(sum(io.toast_blks_read), 0)::int8 AS toast_blks_read,
      coalesce(sum(io.toast_blks_hit), 0)::int8 AS toast_blks_hit
 FROM ranked AS r
 JOIN pg_class AS p ON p.oid = r.oid
 JOIN pg_namespace AS n ON n.oid = p.relnamespace
 LEFT JOIN leaves AS l ON l.parent_oid = r.oid
 LEFT JOIN pg_stat_user_tables AS s ON s.relid = l.leaf_relid
 LEFT JOIN pg_statio_user_tables AS io ON io.relid = l.leaf_relid
GROUP BY p.oid, n.nspname, p.relname
ORDER BY total_bytes DESC;
