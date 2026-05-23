// Build script for doltlite-sys.
//
// TRACER STATUS: This crate currently does NOT compile any C source. Once
// `tools/fetch-doltlite.sh` has populated `vendor/{doltlite.c,sqlite3.h}`,
// enable the `vendored` feature and uncomment the cc::Build block below.
//
// The amalgamation is generated from https://github.com/dolthub/doltlite via:
//   ./configure && make sqlite3.c sqlite3.h
// (see tools/fetch-doltlite.sh).
//
// Doltlite preserves SQLite's C ABI: the symbols are still `sqlite3_*`, so
// this crate exports them under those names. That means it cannot coexist
// in the same process image as a real SQLite linkage; the migration helper
// at `rslib/anki-migrate-sqlite` lives in its own binary for that reason.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=vendor/");

    #[cfg(feature = "vendored")]
    {
        let vendor = std::path::Path::new("vendor");
        let src = vendor.join("doltlite.c");
        assert!(
            src.exists(),
            "doltlite-sys: vendor/doltlite.c missing. Run tools/fetch-doltlite.sh."
        );
        cc::Build::new()
            .file(&src)
            .include(vendor)
            .define("SQLITE_ENABLE_FTS5", None)
            .define("SQLITE_DQS", Some("0"))
            .define("SQLITE_THREADSAFE", Some("1"))
            .compile("doltlite");
    }
}
