-- In-flight DDL & maintenance progress (v0.17).
-- Unifies pg_stat_progress_create_index, pg_stat_progress_cluster, and pg_stat_progress_analyze.
-- Collected on the FAST tick, best-effort.
SELECT
    p.pid,
    coalesce(p.command, 'CREATE INDEX')::text AS command,
    coalesce(c.relname::text, '?') AS relation,
    p.phase::text AS phase,
    case
        when p.blocks_total > 0 then (p.blocks_done::float8 / p.blocks_total::float8 * 100.0)
        when p.tuples_total > 0 then (p.tuples_done::float8 / p.tuples_total::float8 * 100.0)
        when p.lockers_total > 0 then (p.lockers_done::float8 / p.lockers_total::float8 * 100.0)
        when p.partitions_total > 0 then (p.partitions_done::float8 / p.partitions_total::float8 * 100.0)
        else null
    end::float8 AS progress_pct,
    case
        when p.blocks_total > 0 then p.blocks_done
        when p.tuples_total > 0 then p.tuples_done
        when p.lockers_total > 0 then p.lockers_done
        when p.partitions_total > 0 then p.partitions_done
        else 0
    end::int8 AS current_step,
    case
        when p.blocks_total > 0 then p.blocks_total
        when p.tuples_total > 0 then p.tuples_total
        when p.lockers_total > 0 then p.lockers_total
        when p.partitions_total > 0 then p.partitions_total
        else 0
    end::int8 AS total_step,
    coalesce(ic.relname::text, '')::text AS detail
FROM pg_stat_progress_create_index p
LEFT JOIN pg_class c ON c.oid = p.relid
LEFT JOIN pg_class ic ON ic.oid = p.index_relid

UNION ALL

SELECT
    p.pid,
    coalesce(p.command, 'CLUSTER')::text AS command,
    coalesce(c.relname::text, '?') AS relation,
    p.phase::text AS phase,
    case
        when p.heap_blks_total > 0 then (p.heap_blks_scanned::float8 / p.heap_blks_total::float8 * 100.0)
        else null
    end::float8 AS progress_pct,
    coalesce(p.heap_blks_scanned, 0)::int8 AS current_step,
    coalesce(p.heap_blks_total, 0)::int8 AS total_step,
    case
        when p.cluster_index_relid is not null and p.cluster_index_relid > 0 then coalesce(ic.relname::text, '')
        else ''
    end::text AS detail
FROM pg_stat_progress_cluster p
LEFT JOIN pg_class c ON c.oid = p.relid
LEFT JOIN pg_class ic ON ic.oid = p.cluster_index_relid

UNION ALL

SELECT
    p.pid,
    'ANALYZE'::text AS command,
    coalesce(c.relname::text, '?') AS relation,
    p.phase::text AS phase,
    case
        when p.sample_blks_total > 0 then (p.sample_blks_scanned::float8 / p.sample_blks_total::float8 * 100.0)
        else null
    end::float8 AS progress_pct,
    coalesce(p.sample_blks_scanned, 0)::int8 AS current_step,
    coalesce(p.sample_blks_total, 0)::int8 AS total_step,
    ''::text AS detail
FROM pg_stat_progress_analyze p
LEFT JOIN pg_class c ON c.oid = p.relid;
