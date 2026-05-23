//! Rusqlite-shaped binding for Doltlite.
//!
//! ## Tracer-bullet status
//!
//! Until `doltlite-sys` vendors the real Doltlite amalgamation, this crate
//! re-exports `rusqlite` verbatim. That lets us migrate call sites today
//! (`use rusqlite::Foo` → `use doltlite::Foo`) and swap in the real
//! binding later without touching any call site.
//!
//! When the real binding lands, this file becomes a thin wrapper around
//! `doltlite-sys` mirroring rusqlite's API surface (the features Anki
//! needs: `trace`, `functions`, `collation`).

#[cfg(feature = "tracer-stub")]
pub use rusqlite::*;
