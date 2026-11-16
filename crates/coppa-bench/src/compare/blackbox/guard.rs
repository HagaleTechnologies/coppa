//! The repository may publish only a verified coppad self-test.

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

pub fn default_out_dir(name: &str) -> PathBuf {
    super::super::repo_root()
        .join("target/compare-foreign")
        .join(name)
}

fn absolute(path: &Path) -> Result<PathBuf> {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut resolved = PathBuf::new();
    for component in joined.components() {
        match component {
            std::path::Component::ParentDir => {
                resolved.pop();
            }
            std::path::Component::CurDir => {}
            other => {
                resolved.push(other.as_os_str());
                // Resolve each existing component before applying a later `..`.
                // Otherwise a symlink into docs/ can hide a published target.
                if resolved.exists() {
                    resolved = resolved.canonicalize()?;
                }
            }
        }
    }
    Ok(resolved)
}

pub fn check(out_dir: &Path, adapter: &str, version_reply: Option<&str>) -> Result<()> {
    let root = super::super::repo_root().canonicalize()?;
    let target = absolute(out_dir)?;
    let published =
        target.starts_with(root.join("results")) || target.starts_with(root.join("docs"));
    if !published {
        return Ok(());
    }
    if adapter == "kiss" {
        bail!("the kiss adapter has no version reply, so its results cannot go inside this repository");
    }
    let reply = version_reply.unwrap_or("");
    if !reply.starts_with("VERSION Coppa") {
        bail!("the TNC answered {reply:?}, not coppa; results of other modems stay outside this repository");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn foreign_results_cannot_target_repo_paths() {
        let path = super::super::super::repo_root().join("results/compare");
        assert!(check(&path, "vara", Some("VERSION Other 1.2")).is_err());
    }
    #[test]
    fn coppa_version_reply_may_target_results() {
        let path = super::super::super::repo_root().join("results/compare");
        assert!(check(&path, "vara", Some("VERSION Coppa 0.16.0")).is_ok());
    }
    #[test]
    fn kiss_cannot_target_repo_paths() {
        let path = super::super::super::repo_root().join("results/compare");
        assert!(check(&path, "kiss", None).is_err());
    }
    #[test]
    fn default_out_dir_is_under_target() {
        assert!(default_out_dir("coppad").ends_with("target/compare-foreign/coppad"));
    }
    #[test]
    fn relative_nonexistent_output_is_resolved() {
        assert!(check(Path::new("target/new-blackbox-results"), "vara", None).is_ok());
        let path = Path::new("../../target/new/../../results/compare");
        assert!(
            check(path, "vara", Some("VERSION Other")).is_err(),
            "resolved to {}",
            absolute(path).unwrap().display()
        );
    }
    #[cfg(unix)]
    #[test]
    fn symlink_then_parent_cannot_escape_published_guard() {
        let root = super::super::super::repo_root();
        let link = root.join("target/coppa-blackbox-guard-link");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(root.join("docs/benchmarks"), &link).unwrap();
        let path = link.join("../compare-data");
        let rejected = check(&path, "vara", Some("VERSION Other")).is_err();
        std::fs::remove_file(link).unwrap();
        assert!(rejected);
    }
}
