-- I/O profile (v0.16, PG 16+): per-backend_type x context aggregate of
-- `pg_stat_io`, PostgreSQL's own new-in-16 catalog view (no external
-- precedent to adapt — this is the source, not a derivative).
--
-- The raw view is backend_type x object x context (dozens of rows, most
-- all-zero on a quiet server), so this aggregates OVER `object` (relation
-- vs. other) and keeps only rows with at least one nonzero counter — a
-- short, scannable list rather than a wall of zeros.
--
-- `track_io_timing_on` rides along as a per-row scalar (constant across
-- every row of one collection): when the GUC is off, `read_time`/
-- `write_time` read back as 0, indistinguishable from "no time spent", so
-- the Rust parser nulls both out in that case rather than shipping a
-- misleading zero — same convention as `statements.sql`'s
-- `track_io_timing_on`.
--
-- `pg_stat_io`'s per-row counters are NULL (not 0) for a (backend_type,
-- object, context) combination the operation genuinely never applies to
-- (e.g. `extends`/`fsyncs` for most contexts) — verified live against a
-- PG16 container. `sum()` of an all-NULL group is itself NULL, which would
-- silently defeat the `HAVING` filter below (`NULL > 0` is unknown, so
-- EVERY row would be dropped) — every counter is `coalesce`d to 0 first so
-- both the output and the HAVING sum stay well-defined.
SELECT
    io.backend_type::text AS backend_type,
    io.context::text AS context,
    sum(coalesce(io.reads, 0))::int8 AS reads,
    sum(coalesce(io.writes, 0))::int8 AS writes,
    sum(coalesce(io.writebacks, 0))::int8 AS writebacks,
    sum(coalesce(io.extends, 0))::int8 AS extends,
    sum(coalesce(io.hits, 0))::int8 AS hits,
    sum(coalesce(io.evictions, 0))::int8 AS evictions,
    sum(coalesce(io.reuses, 0))::int8 AS reuses,
    sum(coalesce(io.fsyncs, 0))::int8 AS fsyncs,
    sum(coalesce(io.read_time, 0))::float8 AS read_time_ms,
    sum(coalesce(io.write_time, 0))::float8 AS write_time_ms,
    (current_setting('track_io_timing') = 'on') AS track_io_timing_on
FROM pg_catalog.pg_stat_io io
GROUP BY io.backend_type, io.context
HAVING sum(coalesce(io.reads, 0)) + sum(coalesce(io.writes, 0))
     + sum(coalesce(io.writebacks, 0)) + sum(coalesce(io.extends, 0))
     + sum(coalesce(io.hits, 0)) + sum(coalesce(io.evictions, 0))
     + sum(coalesce(io.reuses, 0)) + sum(coalesce(io.fsyncs, 0)) > 0
ORDER BY
    (sum(coalesce(io.reads, 0)) + sum(coalesce(io.writes, 0)) + sum(coalesce(io.hits, 0))) DESC;
