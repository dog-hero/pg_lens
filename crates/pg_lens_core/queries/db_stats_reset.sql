-- Freshness context for the Index Advisor (F3) and any other pg_stat_*
-- reader: when the connected database's cumulative counters were last
-- zeroed. An `idx_scan = 0` finding means nothing if stats were reset five
-- minutes ago — the Schema Lens header shows this age next to the Unused
-- verdict so operators see the caveat, not just the claim (PRD pillar 6:
-- signal, not verdict).
--
-- pg_stat_database is a shared (cluster-wide) view, one row per database —
-- filtered to current_database() to match the rest of the Schema Lens's
-- per-connected-database scope. stats_reset can be NULL (never reset since
-- the cluster was initialized), hence the ::float8 epoch cast staying an
-- Option all the way to the model.
SELECT EXTRACT(epoch FROM stats_reset)::float8 AS stats_reset
  FROM pg_stat_database
 WHERE datname = current_database();
