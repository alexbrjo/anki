// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

include!("../../build/doltlite_force_load.rs");

fn main() {
    force_load_libdoltlite(env!("CARGO_MANIFEST_DIR"), &["..", "doltlite-sys", "lib"]);
}
