//! pg_lens_core — data models and the PostgreSQL poller shared by every
//! pg_lens frontend (TUI, future web).
//!
//! This crate must stay frontend-agnostic: no terminal/TUI dependencies and
//! no knowledge of any frontend's internal message types.

pub mod db;
pub mod history;
pub mod history_store;
pub mod models;
pub mod poller;
pub mod queries;
pub mod services;
pub mod settings;
pub mod waits;

// Re-exported so frontends can name `tokio_postgres::Config` (the type
// `settings::resolve` returns) without pinning their own copy of the crate.
pub use tokio_postgres;

pub use history::{HistoryPoint, SnapshotHistory};
pub use models::{
    ActivityRow, AdminActionResult, AdminCommand, AdminKind, AdminOutcome, BloatRow, DbSnapshot,
    LockRow, PollerStatus, ReplicationInfo, SchemaSnapshot, SchemaStatus, ServerVitals,
    StatementRow, StatementsSnapshot, StatementsStatus, TableStatRow, VacuumClusterAge,
    VacuumProgressRow, VacuumTableRow, WalReceiverRow, WalSenderRow,
};
pub use services::PasswordSource;
pub use waits::{WaitSummary, top_waits};
pub use settings::{ConnLabel, ConnSpec, Resolved, SettingsError};
