-- User sequences exhaustion tracking (v0.19): pg_sequences joined with pg_depend to
-- associate each sequence with its owning table and column.
-- Runs on the SLOW schema cadence (default 60s) — sequence metadata rarely changes.
--
-- Visibility: User sequences in non-system schemas.
SELECT
    s.schemaname::text AS schemaname,
    s.sequencename::text AS sequencename,
    s.data_type::text AS data_type,
    s.start_value::int8 AS start_value,
    s.min_value::int8 AS min_value,
    s.max_value::int8 AS max_value,
    s.increment_by::int8 AS increment_by,
    s.cycle AS cycle,
    s.last_value::int8 AS last_value,
    coalesce(tbl.relname::text, '') AS table_name,
    coalesce(att.attname::text, '') AS column_name
FROM pg_sequences s
JOIN pg_namespace n ON n.nspname = s.schemaname
JOIN pg_class seq_cls ON seq_cls.relname = s.sequencename AND seq_cls.relnamespace = n.oid AND seq_cls.relkind = 'S'
LEFT JOIN pg_depend dep ON dep.objid = seq_cls.oid AND dep.deptype IN ('a', 'i') AND dep.classid = 'pg_class'::regclass AND dep.refclassid = 'pg_class'::regclass
LEFT JOIN pg_class tbl ON tbl.oid = dep.refobjid
LEFT JOIN pg_attribute att ON att.attrelid = dep.refobjid AND att.attnum = dep.refobjsubid
WHERE n.nspname NOT IN ('pg_catalog', 'information_schema')
ORDER BY
    CASE
        WHEN s.increment_by > 0 AND s.last_value IS NOT NULL AND (s.max_value - s.min_value) > 0
        THEN (s.last_value - s.min_value)::float8 / (s.max_value - s.min_value)::float8
        ELSE 0.0
    END DESC,
    s.schemaname, s.sequencename
LIMIT 100;
