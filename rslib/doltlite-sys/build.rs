//! Build script for doltlite-sys.
//!
//! Links the vendored `libdoltlite.a` produced by
//! `tools/fetch-doltlite.sh` (target: `make doltlite-lib`). That static
//! library is the *real* Doltlite engine: prolly tree backend +
//! content-addressed chunk store + dolt_* SQL functions.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=vendor/libdoltlite.a");

    #[cfg(feature = "vendored")]
    {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let vendor = manifest_dir.join("vendor");
        let lib = vendor.join("libdoltlite.a");
        assert!(
            lib.exists(),
            "doltlite-sys: vendor/libdoltlite.a missing. Run tools/fetch-doltlite.sh."
        );
        println!("cargo:rustc-link-search=native={}", vendor.display());
        println!("cargo:rustc-link-lib=static=doltlite");
        println!("cargo:rustc-link-lib=z");
        if cfg!(not(target_os = "windows")) {
            println!("cargo:rustc-link-lib=pthread");
        }
    }
}
