// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

mod rust_interface;

use std::fs;

use anki_proto_gen::descriptors_path;
use anyhow::Result;
use prost_reflect::DescriptorPool;

fn main() -> Result<()> {
    println!("cargo:rerun-if-changed=../out/buildhash");
    let buildhash = fs::read_to_string("../out/buildhash").unwrap_or_default();
    println!("cargo:rustc-env=BUILDHASH={buildhash}");

    emit_force_load_libdoltlite()?;

    let descriptors_path = descriptors_path();
    println!("cargo:rerun-if-changed={}", descriptors_path.display());
    let pool = DescriptorPool::decode(std::fs::read(descriptors_path)?.as_ref())?;
    rust_interface::write_rust_interface(&pool)?;
    Ok(())
}

/// Force-load every .o in libdoltlite.a into the final binary. Without this,
/// the static linker drops the prolly engine's auto-extension registration
/// because nothing references it directly, and the result is a stock-SQLite
/// build. We emit the flag from rslib's build.rs (not .cargo/config.toml's
/// rustflags) so it only applies to binaries that pull in rslib, not to
/// unrelated build scripts.
fn emit_force_load_libdoltlite() -> Result<()> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")?;
    let archive = std::path::Path::new(&manifest_dir)
        .join("doltlite-sys")
        .join("lib")
        .join("libdoltlite.a");
    let archive_str = archive
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("libdoltlite.a path not utf-8"))?;
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
    Ok(())
}
