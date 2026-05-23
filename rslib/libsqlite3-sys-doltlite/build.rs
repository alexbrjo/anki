//! Build script for the Doltlite-backed libsqlite3-sys spoof.
//!
//! ## Status: SQLite-compat amalgamation, not prolly
//!
//! Compiles `rslib/doltlite-sys/vendor/doltlite.c` — the `make sqlite3.c`
//! amalgamation Doltlite ships for stock-SQLite drop-in use. This is
//! **not** the prolly-tree engine; see
//! `rslib/doltlite-sys/PROLLY_BLOCKER.md` for the upstream limitation
//! that prevents us from linking the real `libdoltlite.a` (Anki
//! depends on custom collations, which `DOLTLITE_PROLLY=1` builds
//! refuse to register).
//!
//! When upstream lifts the restriction, switch this script to:
//!
//!   println!("cargo:rustc-link-search=native=...");
//!   println!("cargo:rustc-link-lib=static=doltlite");
//!   println!("cargo:rustc-link-lib=z");
//!   println!("cargo:rustc-link-lib=pthread");
//!
//! (deleting the cc::Build invocation) and the storage layer
//! `is_prolly_engine()` gates start doing their job.
//!
//! The bindgen file we copy to OUT_DIR is upstream libsqlite3-sys's
//! pre-generated bindings (SQLite 3.49.2). Doltlite preserves the
//! SQLite C ABI across minor versions, so SQLite-3.49 bindings work
//! against Doltlite's SQLite-3.54 base.

use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=sqlite3/bindgen_bundled_version.rs");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let vendor_dir = manifest_dir
        .parent()
        .expect("rslib/")
        .join("doltlite-sys")
        .join("vendor");
    let src = vendor_dir.join("doltlite.c");

    assert!(
        src.exists(),
        "Doltlite amalgamation not found at {}.\n\
         Run `tools/fetch-doltlite.sh` from the project root.",
        src.display()
    );
    println!("cargo:rerun-if-changed={}", src.display());

    let mut cfg = cc::Build::new();
    cfg.file(&src)
        .include(&vendor_dir)
        .define("SQLITE_THREADSAFE", Some("1"))
        .define("SQLITE_ENABLE_FTS5", None)
        .define("SQLITE_ENABLE_RTREE", None)
        .define("SQLITE_ENABLE_DBSTAT_VTAB", None)
        // NOTE: SQLITE_DQS deliberately left at default (3). Anki's
        // generated SQL relies on `""` parsing as an empty string
        // literal (a SQLite-historical quirk), so we cannot enable the
        // strict-ANSI DQS=0 mode here.
        // Anki-historical defaults inherited from rusqlite's bundled build.
        .define("SQLITE_DEFAULT_FOREIGN_KEYS", Some("1"))
        .define("SQLITE_ENABLE_API_ARMOR", None)
        .define("SQLITE_ENABLE_COLUMN_METADATA", None)
        .define("SQLITE_ENABLE_LOAD_EXTENSION", Some("0"))
        .flag_if_supported("-Wno-unused-parameter")
        .flag_if_supported("-Wno-unused-function")
        .flag_if_supported("-Wno-implicit-fallthrough");
    cfg.compile("sqlite3");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let bindings_src = manifest_dir
        .join("sqlite3")
        .join("bindgen_bundled_version.rs");
    let bindings_dst = out_dir.join("bindgen.rs");
    std::fs::copy(&bindings_src, &bindings_dst)
        .expect("Could not copy bindings to OUT_DIR");
}
