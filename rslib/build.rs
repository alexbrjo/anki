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

include!("../build/doltlite_force_load.rs");

fn emit_force_load_libdoltlite() -> Result<()> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")?;
    force_load_libdoltlite(&manifest_dir, &["doltlite-sys", "lib"]);
    Ok(())
}
