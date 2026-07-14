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
-- LIMIT guards against databases with tens of thousands of tables: the top
-- 200 by total size is what the lens can usefully show.
SELECT
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
      coalesce(s.autoanalyze_count, 0) AS autoanalyze_count
 FROM pg_stat_user_tables AS s
ORDER BY pg_total_relation_size(s.relid) DESC
LIMIT 200;
