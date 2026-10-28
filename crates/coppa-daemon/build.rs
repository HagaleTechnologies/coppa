//! Build script for coppa-daemon: records the git commit this `coppad` was
//! built from as `COPPA_GIT_SHA`, for the startup log (COP-50).
//!
//! A `COPPA_GIT_SHA` of 7-40 hex digits in the build environment wins (source
//! tarballs, distro packaging). Otherwise git answers, but only when it tracks
//! this crate, so a copy vendored inside another repository never reports
//! that repository's commit. With neither, the value is `unknown`.

use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn is_sha(s: &str) -> bool {
    (7..=40).contains(&s.len()) && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Ask Cargo to re-run this script when HEAD can have moved. Only existing
/// paths are watched: Cargo treats a missing one as changed on every build.
/// A branch whose ref is not a loose file yet is watched through its parent
/// directory, so the commit that creates the file is still seen.
fn watch_head() {
    let mut paths = vec![
        git(&["rev-parse", "--git-path", "HEAD"]),
        git(&["rev-parse", "--git-path", "packed-refs"]),
    ];
    if let Some(head_ref) = git(&["symbolic-ref", "-q", "HEAD"]) {
        if let Some(ref_path) = git(&["rev-parse", "--git-path", &head_ref]) {
            let parent = Path::new(&ref_path)
                .parent()
                .map(|p| p.display().to_string());
            paths.push(if Path::new(&ref_path).exists() {
                Some(ref_path)
            } else {
                parent
            });
        }
    }
    for path in paths.into_iter().flatten() {
        if Path::new(&path).exists() {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}

fn from_git() -> Option<String> {
    git(&["ls-files", "--error-unmatch", "Cargo.toml"])?;
    watch_head();
    git(&["rev-parse", "--short=12", "HEAD"])
}

fn main() {
    println!("cargo:rerun-if-env-changed=COPPA_GIT_SHA");
    let from_env = std::env::var("COPPA_GIT_SHA")
        .ok()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty());
    if let Some(s) = from_env.as_deref().filter(|s| !is_sha(s)) {
        println!("cargo:warning=ignoring COPPA_GIT_SHA={s:?}: expected 7-40 hex digits");
    }
    let sha = from_env
        .filter(|s| is_sha(s))
        .or_else(from_git)
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=COPPA_GIT_SHA={sha}");
}
