-- SLRU cache stats (PG >= 13): pg_stat_slru.
-- Runs on the FAST tick (2s) — cheap single-digit row catalog read.
-- Frontends compute rates and hit-ratio from deltas.
SELECT
    name::text AS name,
    blks_zeroed::int8 AS blks_zeroed,
    blks_hit::int8 AS blks_hit,
    blks_read::int8 AS blks_read,
    blks_written::int8 AS blks_written,
    blks_exists::int8 AS blks_exists,
    flushes::int8 AS flushes,
    truncates::int8 AS truncates
FROM pg_stat_slru
ORDER BY name;
