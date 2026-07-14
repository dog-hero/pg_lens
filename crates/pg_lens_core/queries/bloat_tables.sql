-- Estimated table (heap) bloat of the connected database.
--
-- Adapted from ioguix/pgsql-bloat-estimation, file table/table_bloat.sql
--   https://raw.githubusercontent.com/ioguix/pgsql-bloat-estimation/master/table/table_bloat.sql
--   (master @ commit 8fde3c9, 2022-08-23)
-- Copyright (c) 2015-2019, Jehan-Guillaume (ioguix) de Rorthais
-- All rights reserved. Licensed under BSD-2-Clause; this derived file
-- retains the original copyright notice per the license terms.
--
-- Original notes kept: executed with a non-superuser role, the query
-- inspects only tables and materialized views you are granted to read.
-- Compatible with PostgreSQL 9.0+ (pg_lens floor is 13).
--
-- This is a STATISTICS-BASED ESTIMATE (pg_stats / reltuples): it needs a
-- reasonably fresh ANALYZE, underestimates TOASTed columns, and includes
-- alignment padding. Rows flagged is_na (name-typed columns, missing
-- stats) are unreliable — the Rust layer nulls their numbers.
--
-- pg_lens adaptations (kept minimal):
--   * output aliased/cast onto BloatRow (schema, name, real_bytes,
--     bloat_bytes, bloat_pct, fillfactor, is_na) with the repo's type
--     conventions (::text for name, ::int8/::float8 for numerics);
--   * current database only (pg_catalog is per-database — the original
--     already never crosses databases; the current_database() column was
--     dropped), system schemas excluded;
--   * ORDER BY estimated bloat DESC, LIMIT 200 (same cap as table_stats);
--     the inner ORDER BY was dropped (useless work under a re-sort).
SELECT schemaname::text AS schema, tblname::text AS name,
  (bs*tblpages)::int8 AS real_bytes,
  (CASE WHEN tblpages - est_tblpages_ff > 0
    THEN (tblpages-est_tblpages_ff)*bs
    ELSE 0
  END)::int8 AS bloat_bytes,
  (CASE WHEN tblpages > 0 AND tblpages - est_tblpages_ff > 0
    THEN 100 * (tblpages - est_tblpages_ff)/tblpages::float
    ELSE 0
  END)::float8 AS bloat_pct,
  fillfactor::int4 AS fillfactor, is_na
FROM (
  SELECT ceil( reltuples / ( (bs-page_hdr)/tpl_size ) ) + ceil( toasttuples / 4 ) AS est_tblpages,
    ceil( reltuples / ( (bs-page_hdr)*fillfactor/(tpl_size*100) ) ) + ceil( toasttuples / 4 ) AS est_tblpages_ff,
    tblpages, fillfactor, bs, tblid, schemaname, tblname, heappages, toastpages, is_na
  FROM (
    SELECT
      ( 4 + tpl_hdr_size + tpl_data_size + (2*ma)
        - CASE WHEN tpl_hdr_size%ma = 0 THEN ma ELSE tpl_hdr_size%ma END
        - CASE WHEN ceil(tpl_data_size)::int%ma = 0 THEN ma ELSE ceil(tpl_data_size)::int%ma END
      ) AS tpl_size, bs - page_hdr AS size_per_block, (heappages + toastpages) AS tblpages, heappages,
      toastpages, reltuples, toasttuples, bs, page_hdr, tblid, schemaname, tblname, fillfactor, is_na
    FROM (
      SELECT
        tbl.oid AS tblid, ns.nspname AS schemaname, tbl.relname AS tblname, tbl.reltuples,
        tbl.relpages AS heappages, coalesce(toast.relpages, 0) AS toastpages,
        coalesce(toast.reltuples, 0) AS toasttuples,
        coalesce(substring(
          array_to_string(tbl.reloptions, ' ')
          FROM 'fillfactor=([0-9]+)')::smallint, 100) AS fillfactor,
        current_setting('block_size')::numeric AS bs,
        CASE WHEN version()~'mingw32' OR version()~'64-bit|x86_64|ppc64|ia64|amd64' THEN 8 ELSE 4 END AS ma,
        24 AS page_hdr,
        23 + CASE WHEN MAX(coalesce(s.null_frac,0)) > 0 THEN ( 7 + count(s.attname) ) / 8 ELSE 0::int END
           + CASE WHEN bool_or(att.attname = 'oid' and att.attnum < 0) THEN 4 ELSE 0 END AS tpl_hdr_size,
        sum( (1-coalesce(s.null_frac, 0)) * coalesce(s.avg_width, 0) ) AS tpl_data_size,
        bool_or(att.atttypid = 'pg_catalog.name'::regtype)
          OR sum(CASE WHEN att.attnum > 0 THEN 1 ELSE 0 END) <> count(s.attname) AS is_na
      FROM pg_attribute AS att
        JOIN pg_class AS tbl ON att.attrelid = tbl.oid
        JOIN pg_namespace AS ns ON ns.oid = tbl.relnamespace
        LEFT JOIN pg_stats AS s ON s.schemaname=ns.nspname
          AND s.tablename = tbl.relname AND s.inherited=false AND s.attname=att.attname
        LEFT JOIN pg_class AS toast ON tbl.reltoastrelid = toast.oid
      WHERE NOT att.attisdropped
        AND tbl.relkind in ('r','m')
        AND ns.nspname NOT LIKE 'pg\_%'
        AND ns.nspname <> 'information_schema'
      GROUP BY 1,2,3,4,5,6,7,8,9,10
    ) AS s
  ) AS s2
) AS s3
ORDER BY bloat_bytes DESC NULLS LAST
LIMIT 200;
