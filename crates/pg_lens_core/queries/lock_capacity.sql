-- Lock-table pressure gauge (v0.11): headroom before the classic
-- "out of shared memory, you might need to increase max_locks_per_transaction"
-- outage. The lock table is a fixed-size shared-memory array sized at
-- postmaster start from `max_locks_per_transaction * (max_connections +
-- max_prepared_transactions)` slots (documented capacity formula, see the
-- Postgres docs for `max_locks_per_transaction`) — once every slot is in
-- use, ANY backend requesting a new lock gets a hard failure, not a wait.
-- Own query, not adapted from pg_activity/dalibo.
--
-- `pg_locks` and `current_setting()` on these GUCs are both world-readable,
-- so this rarely fails — but it is still collected best-effort (see
-- `poller::collect_lock_capacity`): a restricted role or a future GUC
-- rename must degrade to "no gauge this tick", never a poll fault.
--
-- `count(*)` and the three settings all need an explicit ::int8 cast (the
-- well-known tokio-postgres aggregate/text-to-int trap) — current_setting()
-- returns `text`, not an integer type.
SELECT
      (SELECT count(*)::int8 FROM pg_catalog.pg_locks) AS locks_held,
      current_setting('max_locks_per_transaction')::int8 AS max_locks_per_xact,
      current_setting('max_connections')::int8 AS max_connections,
      current_setting('max_prepared_transactions')::int8 AS max_prepared_xacts;
