//! What this `coppad` binary was built from (COP-50).

/// The git commit this binary was built from, or `unknown`; see `build.rs`.
pub const GIT_SHA: &str = env!("COPPA_GIT_SHA");

/// Every coppa-daemon cargo feature and whether this build has it.
/// `features_lists_every_feature_in_cargo_toml` keeps it complete.
pub const FEATURES: [(&str, bool); 6] = [
    ("ardop", cfg!(feature = "ardop")),
    ("cpal-backend", cfg!(feature = "cpal-backend")),
    ("gpio-ptt", cfg!(feature = "gpio-ptt")),
    ("kiss-tnc", cfg!(feature = "kiss-tnc")),
    ("serial-ptt", cfg!(feature = "serial-ptt")),
    ("websocket", cfg!(feature = "websocket")),
];

/// The enabled features, comma-separated, or `none`.
pub fn features() -> String {
    let on: Vec<&str> = FEATURES
        .iter()
        .filter(|(_, enabled)| *enabled)
        .map(|(name, _)| *name)
        .collect();
    if on.is_empty() {
        "none".to_string()
    } else {
        on.join(",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A feature added to Cargo.toml must be added to FEATURES too.
    #[test]
    fn features_lists_every_feature_in_cargo_toml() {
        let manifest: toml::Table = include_str!("../Cargo.toml").parse().unwrap();
        let mut declared: Vec<&str> = manifest["features"]
            .as_table()
            .unwrap()
            .keys()
            .map(String::as_str)
            .filter(|name| *name != "default")
            .collect();
        declared.sort_unstable();
        let listed: Vec<&str> = FEATURES.iter().map(|(name, _)| *name).collect();
        assert_eq!(listed, declared);
    }

    #[test]
    fn git_sha_is_a_commit_hash_or_unknown() {
        assert!(
            GIT_SHA == "unknown"
                || ((7..=40).contains(&GIT_SHA.len())
                    && GIT_SHA.chars().all(|c| c.is_ascii_hexdigit())),
            "{GIT_SHA}"
        );
    }

    #[test]
    fn features_matches_this_build() {
        assert_eq!(
            features().split(',').any(|f| f == "websocket"),
            cfg!(feature = "websocket")
        );
        assert_eq!(
            features().split(',').any(|f| f == "cpal-backend"),
            cfg!(feature = "cpal-backend")
        );
        assert_eq!(features() == "none", FEATURES.iter().all(|(_, on)| !on));
    }
}
