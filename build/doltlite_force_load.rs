// Shared helper: emit per-platform linker flags that whole-archive
// `rslib/doltlite-sys/lib/libdoltlite.a` into the final binary/cdylib.
// `include!`d from every build.rs that produces a binary or cdylib linking
// rslib. Without it, the static linker dead-strips the prolly engine's
// auto-extension registration and the runtime acts as stock SQLite.
//
// `manifest_dir` is the value of CARGO_MANIFEST_DIR at the calling build.rs.
// `archive_rel_components` is the path from there to libdoltlite.a, joined
// at runtime (e.g. &["..", "..", "rslib", "doltlite-sys", "lib"]).

fn force_load_libdoltlite(manifest_dir: &str, archive_rel_components: &[&str]) {
    let mut archive = std::path::PathBuf::from(manifest_dir);
    for c in archive_rel_components {
        archive.push(c);
    }
    archive.push("libdoltlite.a");
    let Some(archive_str) = archive.to_str() else {
        return;
    };
    println!("cargo:rerun-if-changed={archive_str}");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    match target_os.as_str() {
        "macos" | "ios" => {
            println!("cargo:rustc-link-arg=-Wl,-force_load,{archive_str}");
        }
        "linux" | "android" => {
            println!("cargo:rustc-link-arg=-Wl,--whole-archive");
            println!("cargo:rustc-link-arg={archive_str}");
            println!("cargo:rustc-link-arg=-Wl,--no-whole-archive");
        }
        "windows" => {
            println!("cargo:rustc-link-arg=/WHOLEARCHIVE:{archive_str}");
        }
        _ => {}
    }
}
