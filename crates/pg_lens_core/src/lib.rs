//! pg_lens_core — data models (and, in later phases, the PostgreSQL poller)
//! shared by every pg_lens frontend (TUI, future web).
//!
//! This crate must stay frontend-agnostic: no terminal/TUI dependencies and
//! no knowledge of any frontend's internal message types.

pub mod models;
pub mod poller;

pub use models::{ActivityRow, DbSnapshot, PollerStatus, ServerVitals};
