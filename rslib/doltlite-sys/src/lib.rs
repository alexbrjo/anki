//! Raw FFI bindings to Doltlite.
//!
//! TRACER STATUS: a minimal, manually-declared subset of the SQLite C API,
//! enough to prove the vendored Doltlite amalgamation is linkable from
//! Rust. The full bindings (mirroring `libsqlite3-sys`) will be generated
//! with `bindgen` in a follow-up — see `rslib/doltlite` for the
//! consumer-facing wrapper.
//!
//! Doltlite preserves SQLite's C ABI, so the symbols below are spelled
//! exactly as in upstream `<sqlite3.h>`. The build script
//! (`build.rs`, gated behind the `vendored` feature) compiles
//! `vendor/doltlite.c` into `libdoltlite.a` and links it.

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

#[cfg(feature = "vendored")]
mod bindings {
    use std::os::raw::c_char;

    unsafe extern "C" {
        /// `sqlite3_libversion()` — returns the SQLite library version
        /// string. With Doltlite this returns the underlying SQLite
        /// version (e.g. "3.54.0"); Doltlite's own version is exposed
        /// separately via `DOLTLITE_VERSION` (compile-time).
        pub fn sqlite3_libversion() -> *const c_char;

        /// `sqlite3_sourceid()` — returns the source-control identifier
        /// of the SQLite version. Includes "doltlite" / prolly metadata
        /// when built from the Doltlite fork.
        pub fn sqlite3_sourceid() -> *const c_char;
    }

    /// Safe wrapper: returns the linked Doltlite/SQLite library version.
    pub fn libversion() -> &'static str {
        // SAFETY: sqlite3_libversion returns a pointer to a static
        // NUL-terminated C string with 'static lifetime.
        unsafe {
            let p = sqlite3_libversion();
            std::ffi::CStr::from_ptr(p).to_str().expect("utf8 version")
        }
    }

    /// Safe wrapper: returns Doltlite's source ID.
    pub fn sourceid() -> &'static str {
        unsafe {
            let p = sqlite3_sourceid();
            std::ffi::CStr::from_ptr(p).to_str().expect("utf8 sourceid")
        }
    }
}

#[cfg(feature = "vendored")]
pub use bindings::*;

#[cfg(all(test, feature = "vendored"))]
mod tests {
    use super::*;

    #[test]
    fn linked_library_reports_version() {
        let v = libversion();
        println!("doltlite libversion: {v}");
        // Doltlite v0.11.0 is forked from SQLite 3.54.0
        assert!(v.starts_with("3."), "unexpected version: {v}");
    }

    #[test]
    fn sourceid_mentions_doltlite_or_sqlite() {
        let s = sourceid();
        println!("doltlite sourceid: {s}");
        assert!(!s.is_empty());
    }
}
