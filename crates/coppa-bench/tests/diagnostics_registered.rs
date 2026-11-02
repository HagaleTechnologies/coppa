//! COP-90 (H-137): coppa-bench's measurement programs live in `diagnostics/`
//! and are registered one by one as `[[example]]` targets. With
//! `autoexamples = false`, cargo silently skips a file that has no stanza, and
//! `cargo clippy --all-targets` stops compiling it (see `src/cp_arm.rs`), so
//! this test keeps the directory and the manifest in step.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

fn manifest() -> toml::Table {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    fs::read_to_string(root.join("Cargo.toml"))
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn autoexamples_is_off_and_examples_dir_is_gone() {
    let m = manifest();
    assert_eq!(m["package"]["autoexamples"].as_bool(), Some(false));
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    assert!(
        !root.join("examples").exists(),
        "move new programs to diagnostics/"
    );
}

#[test]
fn every_diagnostic_has_exactly_one_example_stanza() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let m = manifest();
    let mut stanzas = BTreeSet::new();
    for e in m["example"].as_array().expect("[[example]] stanzas") {
        let name = e["name"].as_str().unwrap();
        assert_eq!(
            e["path"].as_str(),
            Some(format!("diagnostics/{name}.rs").as_str())
        );
        assert!(stanzas.insert(name.to_owned()), "duplicate stanza {name}");
    }
    let files: BTreeSet<String> = fs::read_dir(root.join("diagnostics"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "rs"))
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(stanzas, files);
}
