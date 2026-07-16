-- Per-table XID age ("vacuum debt") for the worst N user tables of the
-- connected database (F2), joined with pg_stat_user_tables so the
-- dead-tuple ratio the Schema Lens already tracks per table rides along in
-- the same row. Collected on the SLOW schema cadence, same transaction as
-- table_stats — these are cheap catalog reads (pg_class/pg_namespace), not
-- the per-relation size functions that make table_stats itself slow.
--
-- relkind = 'r' (ordinary tables) matches pg_stat_user_tables's own scope,
-- so every row here has a partner in the table-stats collection.
--
-- LIMIT 20: the worst offenders are what operators act on; a full per-table
-- listing already exists in the Tables view via last_(auto)vacuum staleness.
SELECT
      n.nspname::text AS schemaname,
      c.relname::text AS relname,
      age(c.relfrozenxid)::int8 AS age_xids,
      coalesce(s.n_dead_tup, 0)::int8 AS n_dead_tup,
      coalesce(s.n_live_tup, 0)::int8 AS n_live_tup
 FROM pg_class c
 JOIN pg_namespace n ON n.oid = c.relnamespace
 JOIN pg_stat_user_tables s ON s.relid = c.oid
WHERE c.relkind = 'r'
ORDER BY age(c.relfrozenxid) DESC
LIMIT 20;
