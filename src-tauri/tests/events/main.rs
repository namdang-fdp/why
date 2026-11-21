//! Integration tests for the event stores (WHY-12).
//!
//! `conformance.rs` is the contract: ONE set of cases run against BOTH
//! `MemoryEventStore` and `SqliteEventStore`, plus a differential test, so the
//! two can never drift. The other modules test what only the SQLite store has:
//! persistence, concurrency, hostile files, edge-case payloads, the benchmark.
//! Everything lives in temp dirs; nothing touches `$HOME`.

mod common;

mod bench;
mod conformance;
// WHY-64
mod edit_ops;
mod hostile;
mod sqlite;
