-- Schema Lens table stats (PG >= 13): pg_stat_user_tables + on-disk sizes.
-- Runs on the SLOW cadence only (default 60s) — pg_total_relation_size &
-- friends take locks/do lseek per relation and are far too expensive for
-- the 2s activity tick (PLAN_SCHEMA_LENS.md anti-pattern nº 1).
--
-- Column set is the PG13+ stable subset (n_ins_since_vacuum arrived in 13);
-- PG16+ extras (last_seq_scan/last_idx_scan) would be a post_160000 variant.
-- The view is per connected database, not cluster-wide.
--
-- Type notes (tokio-postgres):
--   * schemaname/relname are `name` — cast ::text for a clean String map.
--   * counters are int8; they can be NULL right after a stats reset, so
--     COALESCE to 0 — except idx_scan/idx_tup_fetch, where NULL is a signal
--     ("no indexes / never scanned") the model keeps as Option.
--   * last_* are timestamptz — shipped as epoch seconds ::float8 (the
--     repo-wide convention for time values; EXTRACT(epoch..) is numeric on
--     PG >= 14 but float8 on 13, hence the explicit cast).
--
-- v0.14 added `relid` (the table's `oid`, ::int8 — tokio-postgres has no
-- native `oid` mapping) so the poller can key the per-table size-growth
-- ring on something that survives a rename but correctly resets on a
-- drop+recreate (a fresh oid), unlike schema+name.
--
-- v0.15 (BUG fix — honest table counts + limit fix):
--   * `$1` is the caller-supplied row cap (`schema_table_limit`, default
--     200, configurable via `--schema-table-limit`/config.toml/env, clamped
--     10..10000) — previously a hardcoded `LIMIT` of 200 that silently
--     truncated the list with no way to raise it and no signal that it had.
--     The TRUE total row count is a separate scalar query
--     (`table_stats_total.sql`, run in the same transaction) — see that
--     file's header for why it is not folded into this one.
--   * PERF: naively `ORDER BY pg_total_relation_size(s.relid) DESC LIMIT
--     $1` evaluates the (I/O-touching) size function for EVERY table in
--     the database before the LIMIT trims the output — a 10k-table
--     database pays 10k lseek-class calls every slow tick just to throw
--     away all but N of them. Instead, the `ranked` CTE below picks the
--     top-$1 CANDIDATES using `pg_class.relpages` — a plain in-memory
--     catalog column, no I/O — and the outer query computes the exact
--     `pg_total_relation_size`/`pg_table_size`/`pg_indexes_size` only for
--     those $1 survivors. TRADEOFF: `relpages` is only as fresh as the
--     table's last VACUUM/ANALYZE, so a table that grew explosively since
--     then (e.g. a huge bulk COPY with autovacuum/autoanalyze still
--     pending) could rank slightly low and miss the cut this tick — a
--     stale-ranking risk, not a wrong-answer risk: the sizes reported for
--     whichever rows DO make it through are still the live, exact
--     pg_*_size() calls, never estimated from relpages. Accepted as the
--     right cost/accuracy trade for "which N tables should the dashboard
--     show" on large clusters; the growth ring and vacuum/bloat views are
--     unaffected (bloat estimation already reads relpages directly).
--
-- v0.15 (partition collapsing + drill-down):
--   * `c.relispartition` (-> `is_partition`) flags a row as a LEAF of a
--     native partitioned table. A partitioned-table PARENT
--     (`relkind = 'p'`) is EXCLUDED here (`c.relkind <> 'p'` below) —
--     verified live against PG16 that, contrary to the "parents have no
--     row at all" assumption from older Postgres docs,
--     `pg_stat_user_tables` DOES carry an all-zero row for the parent
--     itself (no physical storage, so every counter reads 0/NULL) — left
--     in, this query would double-report the parent alongside the properly
--     AGGREGATED row `queries/partition_parents.sql` produces, so it is
--     filtered out here regardless of PG version (a no-op filter on a
--     version where the row never existed). Frontends hide
--     `is_partition = true` (leaf) rows by default (collapsed view) and
--     show them on an explicit toggle.
--   * `i.inhparent` (-> `parent_oid`, `NULL` for a non-partition table) is
--     the leaf's IMMEDIATE parent oid from `pg_inherits` — lets frontends
--     group leaves under their parent for the drill-down section without a
--     second round-trip. Multi-level partitioning (a partition that is
--     itself partitioned) reports the immediate parent, which may not be
--     the top-level partitioned table shown as a row in
--     `partition_parents.sql` — an accepted simplification: pg_lens groups
--     one level deep, the common case by far.
WITH ranked AS (
    SELECT s.relid
      FROM pg_stat_user_tables AS s
      JOIN pg_class AS c ON c.oid = s.relid
     WHERE c.relkind <> 'p'
     ORDER BY c.relpages DESC
     LIMIT $1
)
SELECT
      s.relid::int8 AS relid,
      s.schemaname::text AS schemaname,
      s.relname::text AS relname,
      pg_total_relation_size(s.relid) AS total_bytes,
      pg_table_size(s.relid) AS table_bytes,
      pg_indexes_size(s.relid) AS index_bytes,
      coalesce(s.seq_scan, 0) AS seq_scan,
      coalesce(s.seq_tup_read, 0) AS seq_tup_read,
      s.idx_scan,
      s.idx_tup_fetch,
      coalesce(s.n_tup_ins, 0) AS n_tup_ins,
      coalesce(s.n_tup_upd, 0) AS n_tup_upd,
      coalesce(s.n_tup_del, 0) AS n_tup_del,
      coalesce(s.n_tup_hot_upd, 0) AS n_tup_hot_upd,
      coalesce(s.n_live_tup, 0) AS n_live_tup,
      coalesce(s.n_dead_tup, 0) AS n_dead_tup,
      coalesce(s.n_mod_since_analyze, 0) AS n_mod_since_analyze,
      coalesce(s.n_ins_since_vacuum, 0) AS n_ins_since_vacuum,
      EXTRACT(epoch FROM s.last_vacuum)::float8 AS last_vacuum,
      EXTRACT(epoch FROM s.last_autovacuum)::float8 AS last_autovacuum,
      EXTRACT(epoch FROM s.last_analyze)::float8 AS last_analyze,
      EXTRACT(epoch FROM s.last_autoanalyze)::float8 AS last_autoanalyze,
      coalesce(s.vacuum_count, 0) AS vacuum_count,
      coalesce(s.autovacuum_count, 0) AS autovacuum_count,
      coalesce(s.analyze_count, 0) AS analyze_count,
      coalesce(s.autoanalyze_count, 0) AS autoanalyze_count,
      c.relispartition AS is_partition,
      i.inhparent::int8 AS parent_oid
 FROM pg_stat_user_tables AS s
 JOIN ranked AS r ON r.relid = s.relid
 JOIN pg_class AS c ON c.oid = s.relid
 LEFT JOIN pg_inherits AS i ON i.inhrelid = s.relid
ORDER BY pg_total_relation_size(s.relid) DESC;
