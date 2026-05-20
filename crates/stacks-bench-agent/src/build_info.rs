//! Build-time constants populated by `build.rs`. Single source of truth
//! for the sbagent commit SHA the binary was built from. Recorded in
//! each `sessions.jsonl` ledger entry as a permanent audit anchor.

/// HEAD commit SHA of the sbagent workspace at build time, or the
/// literal `"unknown"` when the build happened outside a git checkout
/// (packaged tarball, shallow CI clone without refs). Callers that need
/// to distinguish present-vs-unknown use [`sbagent_git_sha`].
pub const SBAGENT_GIT_SHA_RAW: &str = env!("SBAGENT_GIT_SHA");

/// HEAD commit SHA when known, `None` when the build couldn't determine
/// it. Prefer this over [`SBAGENT_GIT_SHA_RAW`] in serialized payloads
/// so consumers can tell missing from present.
pub fn sbagent_git_sha() -> Option<&'static str> {
    if SBAGENT_GIT_SHA_RAW == "unknown" { None } else { Some(SBAGENT_GIT_SHA_RAW) }
}

/// Crate version as declared in `Cargo.toml`. Pairs with
/// [`sbagent_git_sha`] in the ledger: version anchors the public
/// release identity, sha anchors the exact source tree.
pub const SBAGENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sbagent_version_is_populated() {
        assert!(!SBAGENT_VERSION.is_empty());
    }

    #[test]
    fn sbagent_git_sha_either_resolves_or_is_unknown_sentinel() {
        // Acceptance: 40-char hex sha OR the explicit "unknown" sentinel.
        // The literal "unknown" is the fallback path inside build.rs and
        // is treated as None by `sbagent_git_sha()`.
        match sbagent_git_sha() {
            Some(sha) => {
                assert_eq!(sha.len(), 40, "git rev-parse HEAD returns a 40-char sha: {sha}");
                assert!(
                    sha.chars()
                        .all(|c| c.is_ascii_hexdigit()),
                    "non-hex sha: {sha}"
                );
            }
            None => {
                assert_eq!(SBAGENT_GIT_SHA_RAW, "unknown");
            }
        }
    }
}
