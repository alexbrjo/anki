//! Build script for doltlite-sys.
//!
//! ## Status: linking against Doltlite's SQLite-compat amalgamation, not prolly
//!
//! We currently compile the `make sqlite3.c` amalgamation (stock-SQLite
//! shape, alt1 sourceid). This is **not** the prolly-tree engine — see
//! `PROLLY_BLOCKER.md` for the upstream limitation that prevents us
//! linking the real `libdoltlite.a`. When upstream lifts the
//! collation-in-prolly restriction, swap the cc::Build block below
//! for a `cargo:rustc-link-lib=static=doltlite` directive pointed at
//! `vendor/libdoltlite.a` (kept in `vendor/` alongside).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=vendor/");

    #[cfg(feature = "vendored")]
    {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let vendor = manifest_dir.join("vendor");
        let src = vendor.join("doltlite.c");
        assert!(
            src.exists(),
            "doltlite-sys: vendor/doltlite.c missing. Run tools/fetch-doltlite.sh."
        );
        cc::Build::new()
            .file(&src)
            .include(&vendor)
            .define("SQLITE_ENABLE_FTS5", None)
            .define("SQLITE_THREADSAFE", Some("1"))
            .compile("doltlite");
    }
}
