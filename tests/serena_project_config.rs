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
        // Same header set `block_scalar_body` accepts: literal or folded, with
        // or without a chomping/indentation indicator (`|-`, `>-`, `|2`).
        if value.starts_with('|') || value.starts_with('>') {
            in_block_scalar = true;
            scalars.insert(key.to_string(), String::new());
            continue;
        }
        scalars.insert(key.to_string(), unquote(value));
    }
    (scalars, lists)
}

/// Extracts a block-scalar (`key: |` or the equally valid `key: >`) body -- the indented lines that follow,
/// stopping at the first non-indented, non-empty line. `parse_flat_yaml`
/// deliberately does not capture this (see its own doc); a test that needs to
/// check a block scalar's actual prose uses this rather than searching the
/// whole file, which would collateral-match unrelated comments or lists that
/// happen to share a substring.
fn block_scalar_body(src: &str, key: &str) -> String {
    // Accepts every block-scalar header YAML allows here -- literal (`|`) and
    // folded (`>`), with or without a chomping/indentation indicator (`|-`,
    // `>-`, `|+`, `>2`) -- rather than only the literal `key: |` the committed
    // file happens to use today (COP-11 code-review finding F2). Hardcoding
    // `"{key}: |"` meant rewriting `initial_prompt: |` as the equally valid
    // `initial_prompt: >` made every caller panic with "block scalar not
    // found" instead of asserting on the prose it was handed.
    let prefix = format!("{key}:");
    let mut lines = src.lines();
    let mut saw_key = false;
    let mut found_block = false;
    for line in lines.by_ref() {
        let Some(rest) = line.strip_prefix(&prefix) else {
            continue;
        };
        saw_key = true;
        let indicator = rest.split('#').next().unwrap_or(rest).trim();
        if indicator.starts_with('|') || indicator.starts_with('>') {
            found_block = true;
            break;
        }
    }
    assert!(
        found_block,
        "{key} block scalar not found{}",
        if saw_key {
            " -- the key is present but is not a `|`/`>` block scalar"
        } else {
            ""
        }
    );
    let mut body = String::new();
    for line in lines {
        if line.is_empty() || line.starts_with(char::is_whitespace) {
            body.push_str(line);
            body.push('\n');
        } else {
            break;
        }
    }
    body
}

/// Extracts the quoted entries of `Cargo.toml`'s `[workspace] members = [...]`
/// array, so this test tracks the real member list instead of a copy that can
/// silently go stale when a crate is added.
///
/// Scans line-by-line and strips each line's `#` comment before looking for
/// the array's closing `]`, so a comment containing a literal `]` (COP-11
/// code-review finding 2) can't be mistaken for the real terminator and
/// truncate the parsed member list early.
fn workspace_members(cargo_toml: &str) -> Vec<String> {
    let start = cargo_toml
        .find("members = [")
        .expect("Cargo.toml should declare a [workspace] members array");
    let mut array_body = String::new();
    let mut terminated = false;
    for line in cargo_toml[start..].lines() {
        let code = line.split('#').next().unwrap_or(line);
        match code.find(']') {
            Some(end) => {
                array_body.push_str(&code[..end]);
                terminated = true;
                break;
            }
            None => {
                array_body.push_str(code);
                array_body.push('\n');
            }
        }
    }
    assert!(terminated, "the members array should be terminated");
    array_body
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

/// Matches one `/`-separated path segment against an `ignored_paths`-style
/// pattern that may contain at most one meaningful `*` wildcard (e.g. `co*` or
/// `*`). Not a general glob engine -- just enough to keep the shadowing guard
/// below honest about wildcard entries instead of silently skipping them.
fn glob_segment_matches(pattern: &str, segment: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == segment,
        Some((prefix, suffix)) => {
            segment.len() >= prefix.len() + suffix.len()
                && segment.starts_with(prefix)
                && segment.ends_with(suffix)
        }
    }
}

/// Whether an `ignored_paths` entry (`pattern`) would hide a workspace member
/// rooted at `member` from Serena's indexer -- i.e. `pattern` names `member`
/// itself or one of its ancestor directories. Compares path segments rather
/// than raw strings so a trailing-slash pattern (`"crates/"`) is recognised as
/// the same ancestor as `"crates"` instead of silently never matching, and
/// resolves a `*` segment via `glob_segment_matches` instead of skipping any
/// pattern that contains one.
///
/// Follows gitignore matching semantics, which is what Serena actually applies
/// (`pathspec.PathSpec.from_lines(GitWildMatchPattern, ...)`,
/// `serena/project.py:107`): a pattern with an internal `/` is anchored at the
/// project root and must match starting at segment 0, but a slash-free,
/// single-segment pattern (e.g. `"coppa-bench"`, most of this config's
/// entries) matches that name at *any* depth, not just the root -- verified
/// directly against a real `pathspec` install
/// (`GitWildMatchPattern("coppa-bench").match_file("crates/coppa-bench/src/lib.rs")
/// == True`). A `**` path segment (which must stand alone -- git does not
/// special-case `a**` or `**b`) matches zero or more path segments, per the
/// same gitignore spec -- verified against real `pathspec`:
/// `GitWildMatchPattern("crates/**").match_file("crates/coppa-bench") == True`
/// and `GitWildMatchPattern("**/coppa-bench").match_file("crates/coppa-bench")
/// == True` (COP-11 code-review finding 1: the earlier version split on the
/// first literal `*` and treated `**` as two adjacent single-character
/// wildcards, so it silently returned `false` for every `**` pattern).
fn path_pattern_would_shadow(pattern: &str, member: &str) -> bool {
    let pattern_segs: Vec<&str> = pattern
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    let member_segs: Vec<&str> = member
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    if pattern_segs.is_empty() {
        return false;
    }
    if pattern_segs == ["**"] {
        // A lone `**` is unanchored and matches every path.
        return !member_segs.is_empty();
    }
    if pattern_segs.len() == 1 {
        // No internal slash: gitignore matches an unanchored single-segment
        // pattern like this at any depth, not just at the root.
        return (0..member_segs.len())
            .any(|start| glob_segment_matches(pattern_segs[0], member_segs[start]));
    }
    // An internal slash anchors the pattern at the project root.
    segments_match(&pattern_segs, &member_segs)
}

/// Matches anchored pattern segments against path segments, honouring a `**`
/// segment as "zero or more path segments" (tried at every split point so it
/// works as a prefix, infix, or suffix of the pattern: `**/x`, `a/**/x`,
/// `a/**`). Once the pattern is fully consumed, the match succeeds regardless
/// of any leftover path segments -- an anchored pattern that names an
/// ancestor directory shadows everything below it too, matching
/// `path_pattern_would_shadow`'s pre-`**` behaviour of comparing only as many
/// segments as the pattern has.
fn segments_match(pattern_segs: &[&str], segs: &[&str]) -> bool {
    match pattern_segs.split_first() {
        None => true,
        Some((&"**", rest)) => (0..=segs.len()).any(|skip| segments_match(rest, &segs[skip..])),
        Some((head, rest)) => {
            !segs.is_empty()
                && glob_segment_matches(head, segs[0])
                && segments_match(rest, &segs[1..])
        }
    }
}

/// Whether an `ls_workspace_folders` entry covers a workspace member -- i.e.
/// the entry is the project root (`.`) or an ancestor of (or equal to) the
/// member's path.
///
/// `ls_workspace_folders` is Serena's OTHER index-scoping knob, independent of
/// `ignored_paths`: narrowing it (e.g. to `["crates/coppa-dsp"]`, a documented
/// monorepo pattern) hides every other member from symbol search while leaving
/// `ignored_paths` untouched -- reintroducing the silent grep-fallback COP-11
/// exists to prevent, by a route the `ignored_paths`-only guard cannot see
/// (COP-11 code-review finding F1).
fn workspace_folder_covers(folder: &str, member: &str) -> bool {
    let normalise = |p: &str| {
        p.trim()
            .trim_start_matches("./")
            .trim_matches('/')
            .to_string()
    };
    let folder = normalise(folder);
    let member = normalise(member);
    // "." (or "") is the project root and covers every member.
    if folder.is_empty() || folder == "." {
        return true;
    }
    member == folder || member.starts_with(&format!("{folder}/"))
}

/// Guards against `workspace_members` regressing to zero matches (e.g. a
/// `Cargo.toml` reformat past its quote-splitting), which would otherwise let
/// every `for member in workspace_members(...)` loop below iterate zero times
/// and pass its test vacuously instead of guarding anything.
fn assert_members_parsed(members: &[String]) {
    assert!(
        members.iter().any(|m| m == "crates/coppa-protocol"),
        "member parsing failed -- expected crates/coppa-protocol in {members:?}; \
         a parse regression here would make dependent tests pass vacuously by \
         iterating zero members"
    );
}

#[test]
fn workspace_members_ignores_a_bracket_inside_a_comment() {
    // Regression for COP-11 code-review finding 2: a naive `find(']')` over
    // the raw array text stops at the first `]` anywhere, including one
    // inside a `#` comment placed after an earlier entry -- silently
    // truncating every member declared after that comment.
    let toml = "[workspace]\nmembers = [\n    \"crates/a\", # see target]\n    \"crates/b\",\n]\n";
    assert_eq!(workspace_members(toml), vec!["crates/a", "crates/b"]);
}

#[test]
fn block_scalar_body_accepts_every_valid_block_header() {
    // Regression for COP-11 code-review finding F2: the marker used to be the
    // hardcoded string `"{key}: |"`, so rewriting the config's
    // `initial_prompt: |` as the equally valid `initial_prompt: >` (or adding a
    // chomping indicator) made every caller panic with "block scalar not found"
    // instead of asserting on the prose.
    for header in ["|", ">", "|-", ">-", "|+", "|2"] {
        let src = format!("other: x\ninitial_prompt: {header}\n  hello\n  world\nnext: y\n");
        let body = block_scalar_body(&src, "initial_prompt");
        assert!(
            body.contains("hello") && body.contains("world"),
            "header {header:?} produced {body:?}"
        );
        assert!(
            !body.contains("next: y"),
            "header {header:?} overran the block"
        );
    }
    // `parse_flat_yaml` must recognise the same headers, or the body's indented
    // lines leak into the surrounding scalar/list maps.
    let (scalars, lists) = parse_flat_yaml("initial_prompt: >-\n  hello\nafter: y\n");
    assert_eq!(scalars.get("after").map(String::as_str), Some("y"));
    assert!(scalars.contains_key("initial_prompt"));
    assert!(!lists.contains_key("hello"));
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
    // search stops working for the whole workspace. Key is `language_servers`
    // (not the legacy `languages`) -- using the legacy key makes
    // `ProjectConfig._load_yaml_dict` treat the file as incomplete and
    // unconditionally re-save it on first activation, stripping every
    // hand-written comment (verified manually against `serena-agent` 1.7.0's
    // `serena/config/serena_config.py` during COP-11's validation; not
    // exercised by an automated test here since CI has no Serena install).
    assert!(lists
        .get("language_servers")
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
    // .catalyst-cache is the catalyst runner's redirected CARGO_HOME (a
    // vendored crates.io registry mirror). It's excluded only via the
    // machine-local .git/info/exclude, which ignore_all_files_in_gitignore
    // does not read -- so it must be listed here explicitly, or a runner
    // container's Serena activation indexes the whole vendored registry.
    assert!(ignored.iter().any(|p| p == ".catalyst-cache"));
}

#[test]
fn serena_project_config_leaves_every_workspace_member_visible_to_the_indexer() {
    let (_, lists) = parse_flat_yaml(&read(".serena/project.yml"));
    let ignored = lists.get("ignored_paths").cloned().unwrap_or_default();
    // Both scoping knobs, not just `ignored_paths`: see
    // `workspace_folder_covers`. `ls_workspace_folders` is asserted for
    // presence elsewhere (FIELDS_THAT_MUST_BE_PRESENT_TO_AVOID_A_REWRITE);
    // here its VALUE has to actually keep the whole workspace in scope.
    let folders = lists
        .get("ls_workspace_folders")
        .cloned()
        .unwrap_or_default();
    let additional = lists
        .get("ls_additional_workspace_folders")
        .cloned()
        .unwrap_or_default();
    let scoped: Vec<String> = folders.into_iter().chain(additional).collect();
    assert!(
        !scoped.is_empty(),
        "ls_workspace_folders must declare at least one folder (the committed \
         config declares `.`) -- an empty list leaves this guard with nothing \
         to check that the language server is actually pointed at the workspace"
    );
    let members = workspace_members(&read("Cargo.toml"));
    assert_members_parsed(&members);
    for member in &members {
        assert!(
            repo_root().join(member).is_dir(),
            "workspace member {member} does not exist on disk"
        );
        for pattern in &ignored {
            assert!(
                !path_pattern_would_shadow(pattern, member),
                "ignored_paths entry {pattern:?} would hide workspace member {member:?} \
                 from Serena's indexer"
            );
        }
        assert!(
            scoped.iter().any(|f| workspace_folder_covers(f, member)),
            "no ls_workspace_folders entry covers workspace member {member:?} \
             (declared: {scoped:?}) -- narrowing that list scopes the language \
             server away from the member, so symbol lookups there silently fall \
             back to grep even though ignored_paths is clean"
        );
    }
}

#[test]
fn workspace_folder_coverage_catches_a_narrowed_ls_workspace_folders() {
    // The committed value: the project root covers every member.
    assert!(workspace_folder_covers(".", "crates/coppa-dsp"));
    assert!(workspace_folder_covers("./", "crates/coppa-dsp"));
    // A subtree entry covers itself and what is under it, nothing else.
    assert!(workspace_folder_covers("crates", "crates/coppa-dsp"));
    assert!(workspace_folder_covers(
        "crates/coppa-dsp",
        "crates/coppa-dsp"
    ));
    assert!(workspace_folder_covers("./crates/", "crates/coppa-dsp"));
    // The narrowing that F1 describes: scoping to one crate must NOT be
    // reported as covering its siblings.
    assert!(!workspace_folder_covers(
        "crates/coppa-dsp",
        "crates/coppa-ml"
    ));
    // A prefix that is not a path-segment ancestor must not match.
    assert!(!workspace_folder_covers(
        "crates/coppa-d",
        "crates/coppa-dsp"
    ));
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
    let prompt = block_scalar_body(&read(".serena/project.yml"), "initial_prompt");
    assert!(
        prompt.contains("read_memory(\"codebase_map\")"),
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
    let members = workspace_members(&read("Cargo.toml"));
    assert_members_parsed(&members);
    let map = read(".serena/memories/codebase_map.md");
    for member in members {
        let name = member.rsplit('/').next().unwrap_or(&member).to_string();
        assert!(
            map.contains(&name),
            "workspace member {name} is missing from the codebase_map memory"
        );
    }
}

#[test]
fn the_initial_prompt_names_every_workspace_member() {
    // codebase_map.md's own drift guard is the_codebase_map_names_every_workspace_member
    // above; initial_prompt restates the same crate list in prose with no
    // equivalent assertion, so a new crate could go unmentioned there while
    // this suite stays green. Mirror that guard here.
    let members = workspace_members(&read("Cargo.toml"));
    assert_members_parsed(&members);
    let prompt = block_scalar_body(&read(".serena/project.yml"), "initial_prompt");
    for member in members {
        let name = member.rsplit('/').next().unwrap_or(&member).to_string();
        assert!(
            prompt.contains(&name),
            "initial_prompt's crate enumeration is missing {name} -- update it \
             alongside Cargo.toml"
        );
    }
}

#[test]
fn shadowing_guard_catches_a_trailing_slash_pattern() {
    // Regression for a false negative: format!("{member}/").starts_with(&format!("{pattern}/"))
    // compared "crates/coppa-protocol/" against "crates//" for a pattern of
    // "crates/", which never prefix-matches -- silently letting a
    // trailing-slash ignored_paths entry hide every crate under it.
    assert!(path_pattern_would_shadow(
        "crates/",
        "crates/coppa-protocol"
    ));
}

#[test]
fn shadowing_guard_catches_a_glob_pattern() {
    // Regression for a false negative: the old loop `continue`d past any
    // ignored_paths entry containing '*', so "crates/*" would never be
    // checked against any workspace member at all.
    assert!(path_pattern_would_shadow(
        "crates/*",
        "crates/coppa-protocol"
    ));
    assert!(!path_pattern_would_shadow(
        "crates/co*x",
        "crates/coppa-protocol"
    ));
}

#[test]
fn shadowing_guard_does_not_false_positive_on_a_sibling_prefix() {
    // "crates/coppa" must not be treated as shadowing "crates/coppa-protocol"
    // -- segment comparison, not raw string prefix comparison.
    assert!(!path_pattern_would_shadow(
        "crates/coppa",
        "crates/coppa-protocol"
    ));
}

#[test]
fn shadowing_guard_catches_a_bare_name_pattern_at_any_depth() {
    // Regression for a false negative found in validation: a slash-free
    // pattern like "coppa-bench" was only checked anchored at segment 0, so it
    // never matched a member nested one level down. Serena resolves
    // ignored_paths with gitignore semantics (pathspec.PathSpec.from_lines
    // (GitWildMatchPattern, ...), serena/project.py:107), where a single-
    // segment pattern matches that name at ANY depth -- confirmed directly
    // against a real pathspec install:
    // GitWildMatchPattern("coppa-bench").match_file("crates/coppa-bench/src/lib.rs")
    // == True.
    assert!(path_pattern_would_shadow(
        "coppa-bench",
        "crates/coppa-bench"
    ));
    // A multi-segment (slash-containing) pattern stays anchored at the root --
    // it must NOT match the same way a bare name would.
    assert!(!path_pattern_would_shadow(
        "other/coppa-bench",
        "crates/coppa-bench"
    ));
}

#[test]
fn shadowing_guard_catches_double_star_patterns() {
    // Regression for COP-11 code-review finding 1: `glob_segment_matches`
    // split on the first literal `*`, so a `**` segment never got its
    // gitignore meaning ("zero or more path segments") and every pattern
    // below silently returned `false` instead of shadowing the member --
    // confirmed against a real `pathspec` install
    // (GitWildMatchPattern("crates/**").match_file("crates/coppa-bench") and
    // GitWildMatchPattern("**/coppa-bench").match_file("crates/coppa-bench")
    // both == True).
    assert!(path_pattern_would_shadow("crates/**", "crates/coppa-bench"));
    assert!(path_pattern_would_shadow(
        "**/coppa-bench",
        "crates/coppa-bench"
    ));
    assert!(path_pattern_would_shadow("**", "crates/coppa-bench"));
    // `**` must still only stand for path segments -- it must not let
    // "crates/**" shadow a sibling directory that "**" happens to also
    // literally appear before.
    assert!(!path_pattern_would_shadow("tools/**", "crates/coppa-bench"));
}

/// Every field name `ProjectConfig` treats as having no default (per
/// `serena/config/serena_config.py`'s `dataclasses.fields(cls)` minus
/// `FIELDS_WITHOUT_DEFAULTS = {"project_name", "language_servers"}`), plus
/// those two themselves. If any of these keys is missing, or a legacy
/// `RENAMED_FIELDS` key (`languages`, `additional_workspace_folders`) is used
/// instead, `ProjectConfig._load_yaml_dict` sets `was_complete = False` and
/// `ProjectConfig.load` unconditionally re-saves the file -- stripping every
/// hand-written comment and renaming the key in place. The actual reproduction
/// against a real serena-agent install (`was_complete == True`, no rewrite,
/// `git status` clean afterward) was run manually during COP-11's validation,
/// not as an automated test here -- CI has no Serena/rust-analyzer install to
/// run it against. This test pins the field-name shape statically so a future
/// edit can't reintroduce the legacy names without CI catching it.
const FIELDS_THAT_MUST_BE_PRESENT_TO_AVOID_A_REWRITE: &[&str] = &[
    "project_name",
    "language_servers",
    "ignored_paths",
    "ls_workspace_folders",
    "ls_additional_workspace_folders",
    "read_only",
    "ignore_all_files_in_gitignore",
    "initial_prompt",
    "encoding",
    "activation_command",
    "activation_command_timeout",
    "symbol_info_budget",
    "language_backend",
    "line_ending",
    "read_only_memory_patterns",
    "ignored_memory_patterns",
    "ls_specific_settings",
    "default_modes",
    "added_modes",
    "excluded_tools",
    "included_optional_tools",
    "fixed_tools",
];

#[test]
fn serena_project_config_has_every_field_needed_to_avoid_a_rewrite() {
    let (scalars, lists) = parse_flat_yaml(&read(".serena/project.yml"));
    for field in FIELDS_THAT_MUST_BE_PRESENT_TO_AVOID_A_REWRITE {
        assert!(
            scalars.contains_key(*field) || lists.contains_key(*field),
            "project.yml is missing `{field}` -- ProjectConfig._load_yaml_dict \
             would treat the config as incomplete and re-save it on first \
             activation, stripping comments (COP-11 Finding 1)"
        );
    }
    // The legacy, pre-rename key names must NOT appear -- their presence is
    // exactly what makes `ProjectConfig.RENAMED_FIELDS` trigger a rewrite.
    assert!(
        !scalars.contains_key("languages") && !lists.contains_key("languages"),
        "project.yml must use `language_servers`, not the legacy `languages` \
         key (RENAMED_FIELDS) -- see FIELDS_THAT_MUST_BE_PRESENT_TO_AVOID_A_REWRITE"
    );
    assert!(
        !scalars.contains_key("additional_workspace_folders")
            && !lists.contains_key("additional_workspace_folders"),
        "project.yml must use `ls_additional_workspace_folders`, not the legacy \
         `additional_workspace_folders` key (RENAMED_FIELDS)"
    );
}
