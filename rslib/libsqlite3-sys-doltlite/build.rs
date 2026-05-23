//! Build script for the Doltlite-backed libsqlite3-sys spoof.
//!
//! Links the vendored `libdoltlite.a` (built via
//! `tools/fetch-doltlite.sh` → `make doltlite-lib`). That library
//! includes the full prolly-tree engine and exposes the standard
//! `sqlite3_*` C ABI, so rusqlite links unchanged on top of it.
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
    let lib = vendor_dir.join("libdoltlite.a");
    assert!(
        lib.exists(),
        "Doltlite library not found at {}.\n\
         Run `tools/fetch-doltlite.sh` from the project root.",
        lib.display()
    );
    println!("cargo:rerun-if-changed={}", lib.display());

    println!("cargo:rustc-link-search=native={}", vendor_dir.display());
    println!("cargo:rustc-link-lib=static=doltlite");
    println!("cargo:rustc-link-lib=z");
    if cfg!(not(target_os = "windows")) {
        println!("cargo:rustc-link-lib=pthread");
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let bindings_src = manifest_dir
        .join("sqlite3")
        .join("bindgen_bundled_version.rs");
    let bindings_dst = out_dir.join("bindgen.rs");
    std::fs::copy(&bindings_src, &bindings_dst)
        .expect("Could not copy bindings to OUT_DIR");
}
