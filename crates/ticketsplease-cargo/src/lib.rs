//! Rust/cargo crate-graph backend for the ticketsplease conflict guard.
//!
//! Maps changed files to affected scopes by walking the cargo crate graph
//! (reverse-dependents), so a change to a leaf crate flags its dependents.
//! Implemented at milestone M4; requires `cargo` on `PATH` at runtime.
