//! Raw FFI bindings to Doltlite.
//!
//! TRACER STATUS: empty until the amalgamation is vendored. The real
//! contents will be a near-verbatim copy of `libsqlite3-sys`'s generated
//! bindings, retargeted at `doltlite.c` instead of `sqlite3.c`.
//!
//! Doltlite preserves SQLite's C ABI, so consumers can use the same
//! `sqlite3_*` symbol names that `libsqlite3-sys` exposes. The Rust
//! binding crate at `rslib/doltlite` wraps these in a rusqlite-shaped API.

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]
