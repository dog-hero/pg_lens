-- Logical replication publications (pg_publication + table counts).
--
-- Lists publications configured in the current database, operations published
-- (insert/update/delete/truncate), whether all tables are published, and total
-- published tables count from pg_publication_tables.
--
-- Version-independent across PG 13+: pg_publication and pubviaroot exist since
-- PG 13.
SELECT
      p.pubname::text AS pubname,
      r.rolname::text AS owner,
      p.puballtables,
      p.pubinsert,
      p.pubupdate,
      p.pubdelete,
      p.pubtruncate,
      p.pubviaroot,
      count(t.tablename)::int8 AS table_count,
      string_agg(t.schemaname || '.' || t.tablename, ', ' ORDER BY t.schemaname, t.tablename) AS published_tables
 FROM pg_publication p
 JOIN pg_roles r ON r.oid = p.pubowner
 LEFT JOIN pg_publication_tables t ON t.pubname = p.pubname
GROUP BY p.pubname, r.rolname, p.puballtables, p.pubinsert, p.pubupdate, p.pubdelete, p.pubtruncate, p.pubviaroot
ORDER BY p.pubname;
