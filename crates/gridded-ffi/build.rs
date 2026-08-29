//! Generates `apple/include/gridded_ffi.h` from this crate's `#[no_mangle]
//! extern "C"` functions via `cbindgen`, and checks the result into the
//! repository (see the crate root and `apple/project.yml`) so that Xcode
//! builds don't depend on running `cargo build` before opening the header.
//!
//! The header is only written when its contents actually change, so a
//! plain `cargo build` doesn't dirty the working tree (and doesn't cause
//! Xcode -- which references the header directly on disk, not through a
//! build phase -- to see spurious modification-time churn).

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let crate_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo");
    let crate_dir = PathBuf::from(crate_dir);

    let header_path = crate_dir
        .join("..")
        .join("..")
        .join("apple")
        .join("include")
        .join("gridded_ffi.h");

    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=cbindgen.toml");
    println!("cargo:rerun-if-changed=build.rs");

    let config = cbindgen::Config::from_file(crate_dir.join("cbindgen.toml"))
        .expect("cbindgen.toml must parse");

    let bindings = match cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(config)
        .generate()
    {
        Ok(bindings) => bindings,
        Err(err) => {
            // Don't fail the whole workspace build (e.g. `cargo test
            // --workspace`, or a build where the checked-in header is
            // already correct and just can't be regenerated in this
            // environment) over a header-generation hiccup; warn instead.
            println!("cargo:warning=gridded-ffi: failed to generate C header: {err}");
            return;
        }
    };

    let mut new_contents = Vec::new();
    bindings.write(&mut new_contents);

    let new_contents = String::from_utf8(new_contents).expect("cbindgen output is valid UTF-8");

    let unchanged = fs::read_to_string(&header_path)
        .map(|existing| existing == new_contents)
        .unwrap_or(false);

    if unchanged {
        return;
    }

    if let Some(parent) = header_path.parent() {
        fs::create_dir_all(parent).expect("failed to create apple/include");
    }
    fs::write(&header_path, new_contents).expect("failed to write apple/include/gridded_ffi.h");
}
