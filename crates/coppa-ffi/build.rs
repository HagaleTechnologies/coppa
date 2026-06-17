//! Build script for coppa-ffi: generates `coppa.h` via cbindgen.
//!
//! The header is written to `$CARGO_MANIFEST_DIR/coppa.h` so it can be
//! checked into source control and used by C/Swift consumers directly.

fn main() {
    // Only regenerate when the source changes.
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=cbindgen.toml");

    let crate_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();

    match cbindgen::generate(&crate_dir) {
        Ok(bindings) => {
            bindings.write_to_file(format!("{}/coppa.h", crate_dir));
        }
        Err(cbindgen::Error::ParseSyntaxError { .. }) => {
            // During `cargo test` the crate may have test-only items that
            // cbindgen cannot parse. Silently skip generation.
            eprintln!("cbindgen: skipping header generation (parse error)");
        }
        Err(e) => {
            eprintln!("cbindgen: warning: {}", e);
        }
    }
}
