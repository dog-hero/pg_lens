-- Index advisor (F3): per-index usage counters + on-disk size, joined with
-- pg_index for uniqueness/constraint flags and the raw catalog signature
-- (indkey/indclass/indcollation/indpred) that `index_advisor::classify`
-- (pure Rust, unit-tested — see crates/pg_lens_core/src/index_advisor.rs)
-- uses to detect exact/prefix duplicates. Collected on the SLOW schema
-- cadence, same essential transaction as table_stats/vacuum_table_ages —
-- pg_stat_user_indexes is a cheap catalog view, no per-relation size scan
-- beyond pg_relation_size itself.
--
-- Base is pg_stat_user_indexes (not pg_index directly): it already scopes
-- to indexes of user tables/matviews in non-system schemas, matching the
-- rest of the Schema Lens's per-connected-database convention.
--
-- Type notes (tokio-postgres):
--   * schemaname/relname/indexrelname are `name` — cast ::text.
--   * idx_scan/idx_tup_read/idx_tup_fetch are int8 but nullable right after
--     a stats reset — COALESCE to 0 (unlike table_stats's idx_scan, an
--     index either exists with counters or it doesn't show up at all here).
--   * indkey/indclass/indcollation are int2vector/oidvector — cast ::text
--     to a space-separated token list; `index_advisor` compares them as
--     opaque strings, never interprets the OIDs.
--   * indpred is a stored node tree; pg_get_expr() decompiles it against
--     the owning table, COALESCEd to '' when the index carries no
--     predicate (a plain, non-partial index — the common case).
--   * a constraint-backed index (PK/UNIQUE/EXCLUDE) has a pg_constraint
--     row whose conindid points back at it — is_constraint flags that,
--     read alongside pg_index's own indisunique/indisprimary/indisexclusion
--     (the flags `index_advisor` actually gates "never flag" on).
--   * indisvalid/indisready: false on either means a `CREATE INDEX
--     CONCURRENTLY` was interrupted (crash, cancel) and left a dead,
--     never-queryable index behind — pure write/disk overhead that `\d`
--     does not warn about. Available on all supported versions (PG 13+),
--     no version gate needed.
--
-- LIMIT 50: the worst-by-size offenders are what an operator acts on first;
-- same row-cap philosophy as table_stats/vacuum_table_ages.
SELECT
      s.schemaname::text AS schemaname,
      s.relname::text AS tablename,
      s.indexrelname::text AS indexname,
      pg_relation_size(s.indexrelid) AS index_bytes,
      coalesce(s.idx_scan, 0)::int8 AS idx_scan,
      coalesce(s.idx_tup_read, 0)::int8 AS idx_tup_read,
      coalesce(s.idx_tup_fetch, 0)::int8 AS idx_tup_fetch,
      idx.indisunique AS is_unique,
      idx.indisprimary AS is_primary,
      idx.indisexclusion AS is_exclusion,
      idx.indisvalid AS is_valid,
      idx.indisready AS is_ready,
      (con.oid IS NOT NULL) AS is_constraint,
      idx.indkey::text AS indkey,
      idx.indclass::text AS indclass,
      idx.indcollation::text AS indcollation,
      coalesce(pg_get_expr(idx.indpred, idx.indrelid), '') AS indpred,
      pg_get_indexdef(idx.indexrelid)::text AS indexdef
 FROM pg_stat_user_indexes s
 JOIN pg_index idx ON idx.indexrelid = s.indexrelid
 LEFT JOIN pg_constraint con ON con.conindid = idx.indexrelid
ORDER BY pg_relation_size(s.indexrelid) DESC
LIMIT 50;
