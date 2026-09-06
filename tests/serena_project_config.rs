//! COP-11: guard the committed Serena project configuration that gives
//! catalyst-dev's codebase-analyzer / codebase-locator / codebase-pattern-finder
//! subagents real symbol search in this repo.
//!
//! Serena activation itself cannot be exercised from `cargo test` -- it needs the
//! `mcp__serena__*` MCP server, which CI does not run. What CI *can* guard is the
//! shape of the committed config: that it exists, parses, names this repo, still
//! declares Rust, stays navigation-only, ignores the noise that would pollute an
//! index, keeps every workspace member visible to the indexer, and ships the
//! `codebase_map` memory the config's own `initial_prompt` tells agents to read.
//! Those are the parts a rename, a move, or a tidy-up would silently break --
//! which would put this repo straight back into the silent grep-fallback state
//! COP-11 was filed for, with nothing to notice.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

/// The root `coppa` package's manifest dir IS the repo root, so no `../..` walk
/// is needed here (contrast `crates/coppa-protocol/tests/golden_vectors.rs`,
/// which lives one crate down and does need one).
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn serena_dir() -> PathBuf {
    repo_root().join(".serena")
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}

fn unquote(value: &str) -> String {
    value
        .trim()
        .trim_matches(|c| c == '\'' || c == '"')
        .to_string()
}

/// Reads the flat `key: value` / `key:` + `- item` / `key: |` block-scalar subset
/// that `.serena/project.yml` uses.
///
/// Deliberately hand-rolled rather than pulling in a YAML crate: the workspace
/// has no YAML dependency today (`[workspace.dependencies]` has `toml` and
/// `serde_json` only), and this repo runs cargo-deny, a RustSec audit job, an
/// MSRV job, and a Dependabot auto-merge lane -- so adding a crate to the
/// dependency graph for a tooling guard test is a real cost, for ~40 lines of
/// parsing over a file this very test pins the shape of.
fn parse_flat_yaml(src: &str) -> (BTreeMap<String, String>, BTreeMap<String, Vec<String>>) {
    let mut scalars: BTreeMap<String, String> = BTreeMap::new();
    let mut lists: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut current: Option<String> = None;
    let mut in_block_scalar = false;

    for raw in src.lines() {
        let line = raw.trim_end();
        if in_block_scalar {
            // A block scalar's body is indented; the first non-indented,
            // non-empty line ends it.
            if line.is_empty() || line.starts_with(char::is_whitespace) {
                continue;
            }
            in_block_scalar = false;
        }
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(item) = trimmed.strip_prefix("- ") {
            if let Some(key) = current.as_deref() {
                lists
                    .entry(key.to_string())
                    .or_default()
                    .push(unquote(item));
            }
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.is_empty() || key.contains(char::is_whitespace) {
            continue;
        }
        current = Some(key.to_string());
        lists.entry(key.to_string()).or_default();
        let value = value.trim();
        if value == "|" || value == ">" {
            in_block_scalar = true;
            scalars.insert(key.to_string(), String::new());
            continue;
        }
        scalars.insert(key.to_string(), unquote(value));
    }
    (scalars, lists)
}

/// Extracts the quoted entries of `Cargo.toml`'s `[workspace] members = [...]`
/// array, so this test tracks the real member list instead of a copy that can
/// silently go stale when a crate is added.
fn workspace_members(cargo_toml: &str) -> Vec<String> {
    let start = cargo_toml
        .find("members = [")
        .expect("Cargo.toml should declare a [workspace] members array");
    let rest = &cargo_toml[start..];
    let end = rest
        .find(']')
        .expect("the members array should be terminated");
    rest[..end]
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

#[test]
fn serena_project_config_is_committed_at_the_repo_root() {
    assert!(
        serena_dir().join("project.yml").is_file(),
        ".serena/project.yml must be committed at the repo root -- without it, \
         mcp__serena__activate_project has nothing to activate and every \
         catalyst-dev symbol lookup silently falls back to grep (COP-11)."
    );
}

#[test]
fn serena_project_config_identifies_this_repo_and_stays_navigation_only() {
    let (scalars, lists) = parse_flat_yaml(&read(".serena/project.yml"));
    assert_eq!(
        scalars.get("project_name").map(String::as_str),
        Some("coppa")
    );
    assert_eq!(scalars.get("read_only").map(String::as_str), Some("true"));
    assert_eq!(
        scalars
            .get("ignore_all_files_in_gitignore")
            .map(String::as_str),
        Some("true")
    );
    // Rust is the only language here; if this ever loses its entry, symbol
    // search stops working for the whole workspace.
    assert!(lists
        .get("languages")
        .is_some_and(|l| l.iter().any(|v| v == "rust")));
}

#[test]
fn serena_project_config_ignores_the_noise_that_would_pollute_an_index() {
    let (_, lists) = parse_flat_yaml(&read(".serena/project.yml"));
    let ignored = lists.get("ignored_paths").cloned().unwrap_or_default();
    assert!(ignored.iter().any(|p| p == ".serena/cache"));
    // testdata/golden is ~1.5 MB of frozen binary WAV regression vectors that
    // .gitignore deliberately UN-ignores (`!testdata/golden/*.wav`), so
    // ignore_all_files_in_gitignore does not cover them. No symbols, no prose.
    assert!(ignored.iter().any(|p| p == "testdata/golden"));
}

#[test]
fn serena_project_config_leaves_every_workspace_member_visible_to_the_indexer() {
    let (_, lists) = parse_flat_yaml(&read(".serena/project.yml"));
    let ignored = lists.get("ignored_paths").cloned().unwrap_or_default();
    let members = workspace_members(&read("Cargo.toml"));
    assert!(
        members.iter().any(|m| m == "crates/coppa-protocol"),
        "member parsing failed -- expected crates/coppa-protocol in {members:?}"
    );
    for member in &members {
        assert!(
            repo_root().join(member).is_dir(),
            "workspace member {member} does not exist on disk"
        );
        for pattern in &ignored {
            if pattern.contains('*') {
                continue; // globs cannot shadow a literal member root here
            }
            assert!(
                !format!("{member}/").starts_with(&format!("{pattern}/")),
                "ignored_paths entry {pattern:?} would hide workspace member {member:?} \
                 from Serena's indexer"
            );
        }
    }
}

#[test]
fn serena_keeps_its_machine_local_halves_out_of_git() {
    let nested = read(".serena/.gitignore");
    assert!(nested.contains("/cache"));
    assert!(nested.contains("/project.local.yml"));
}

#[test]
fn serena_ships_the_memory_its_own_initial_prompt_promises() {
    // The name `codebase_map` is not hardcoded in any catalyst-dev agent
    // definition -- it is a fleet convention (see /catalyst/.serena/project.yml)
    // that becomes load-bearing here because THIS repo's own initial_prompt
    // instructs `read_memory("codebase_map")`. Both halves are asserted so the
    // two can never drift apart.
    let (scalars, _) = parse_flat_yaml(&read(".serena/project.yml"));
    assert!(scalars.contains_key("initial_prompt"));
    let raw = read(".serena/project.yml");
    assert!(
        raw.contains("read_memory(\"codebase_map\")"),
        "initial_prompt must keep pointing at the codebase_map memory"
    );
    assert!(serena_dir()
        .join("memories")
        .join("codebase_map.md")
        .is_file());
}

#[test]
fn the_codebase_map_names_every_workspace_member() {
    // Adding a crate without adding it to the map should fail here rather than
    // quietly leaving agents with a stale directory map.
    let map = read(".serena/memories/codebase_map.md");
    for member in workspace_members(&read("Cargo.toml")) {
        let name = member.rsplit('/').next().unwrap_or(&member).to_string();
        assert!(
            map.contains(&name),
            "workspace member {name} is missing from the codebase_map memory"
        );
    }
}
