//! Build script for the Doltlite-backed libsqlite3-sys spoof.
//!
//! We always compile the vendored Doltlite amalgamation
//! (`rslib/doltlite-sys/vendor/doltlite.c`) and emit upstream
//! libsqlite3-sys's pre-generated bindgen file to OUT_DIR/bindgen.rs.
//! Doltlite preserves SQLite's C ABI, so the bindings (generated from
//! SQLite 3.49.2) are compatible with Doltlite's SQLite 3.54.0 base.
//!
//! Configuration mirrors the C flags Doltlite ships in its own
//! Makefile (FTS5, RTREE, threading) plus the rusqlite-bundled defaults
//! that rslib has historically relied on (column_metadata, threading,
//! enable extension loading off).

use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=sqlite3/bindgen_bundled_version.rs");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    // The amalgamation lives in the sibling -sys crate's vendor dir.
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
        // Match Doltlite's own Makefile defaults.
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
        // Quiet some upstream warnings.
        .flag_if_supported("-Wno-unused-parameter")
        .flag_if_supported("-Wno-unused-function")
        .flag_if_supported("-Wno-implicit-fallthrough");
    cfg.compile("sqlite3"); // produces libsqlite3.a — matches `links = "sqlite3"`

    // Copy the upstream-shipped bindgen file to OUT_DIR for src/lib.rs.
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let bindings_src = manifest_dir
        .join("sqlite3")
        .join("bindgen_bundled_version.rs");
    let bindings_dst = out_dir.join("bindgen.rs");
    std::fs::copy(&bindings_src, &bindings_dst)
        .expect("Could not copy bindings to OUT_DIR");
}
