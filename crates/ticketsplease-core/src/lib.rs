//! ticketsplease-core — git-native ticket model, scheduling, and conflict-guard logic.
//!
//! Intentionally dependency-light and toolchain-free so it compiles into a fully
//! static, portable binary that runs anywhere. The Rust crate-graph backend
//! (which requires `cargo` at runtime) lives in the separate `ticketsplease-cargo`
//! crate and plugs in behind the scheduling/guard core.

pub mod claim;
pub mod comment;
pub mod config;
pub mod error;
pub mod event;
pub mod frontmatter;
pub mod guard;
pub mod ids;
pub mod lint;
pub mod migrate;
pub mod plan;
pub mod query;
pub mod schedule;
pub mod states;
pub mod store;
pub mod ticket;
pub mod txn;
pub mod validate;
pub mod views;

pub use error::{Error, Result};
pub use plan::{BoardSnapshot, CreateSpec, IdAllocator, MutationPlan, PlannedCreate};
pub use states::{Category, StateClass, StateRegistry};
pub use store::{CreateOutcome, Store};
pub use ticket::{ClosedReason, Priority, Ticket};
pub use validate::{validate_plan, validate_ticket_links, ValidationOptions, WriteFields};
