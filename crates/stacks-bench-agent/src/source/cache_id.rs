//! Cache id derivation for the bare object cache.
//!
//! `<agent_workspace_root>/cache/<cache_id>.git/` is keyed by an id
//! derived from the configured `[source].url` when the operator hasn't
//! pinned `source.id` explicitly. The derivation:
//!
//! - Canonicalize the URL: lowercase, strip a leading `git@github.com:` / `ssh://git@github.com/`
//!   / `https://` / `http://` prefix to leave just the host + owner + repo
//!   path, strip a trailing `.git`.
//! - Replace every non-`[a-z0-9-]` char with `-`, collapse runs of `-`, trim
//!   leading + trailing `-`.
//! - Truncate the slug to 55 chars, leaving room for a `-<hash>` suffix (8 hex
//!   chars of SHA-256 over the canonical URL — see [`url_hash_prefix`]).
//!
//! Two different remotes named `stacks-core.git` therefore cannot
//! collide: the SHA-256 prefix differentiates
//! `stacks-network/stacks-core` from any `cylewitruk/stacks-core` fork
//! even when an operator forgets to set `source.id`.
//!
//! Examples:
//! - `https://github.com/stacks-network/stacks-core.git` →
//!   `github-com-stacks-network-stacks-core-<hash>`
//! - `git@github.com:cylewitruk/stacks-core.git` →
//!   `github-com-cylewitruk-stacks-core-<hash>`

use sha2::{Digest as _, Sha256};

use crate::settings::validate_source_id;

/// Resolve the cache id for a session:
///
/// - If the operator pinned `source.id`, use it verbatim (already validated by
///   [`crate::settings::validate_source_id`] at config-load time; re-validated
///   here as defense-in-depth).
/// - Otherwise derive deterministically from `source_url` via
///   [`derive_cache_id`].
///
/// Returns `Err` if a pinned `source.id` somehow slipped past settings
/// validation (e.g. constructed at runtime by a buggy caller).
pub fn resolve_cache_id(pinned: Option<&str>, source_url: &str) -> Result<String, String> {
    match pinned {
        Some(id) => {
            validate_source_id(id)?;
            Ok(id.to_owned())
        }
        None => Ok(derive_cache_id(source_url)),
    }
}

/// Derive a cache id from a clone URL, deterministic for any given
/// input. Output always validates against
/// [`crate::settings::validate_source_id`]'s regex by construction:
/// the slug portion is canonicalised to `[a-z0-9-]`, the hash suffix
/// is `[0-9a-f]`, and the total length is bounded.
pub fn derive_cache_id(source_url: &str) -> String {
    let canonical = canonicalize_url_for_hashing(source_url);
    let slug = sluggify(&canonical);
    let truncated_slug = truncate_slug(&slug, 55);
    let hash = url_hash_prefix(&canonical);
    if truncated_slug.is_empty() {
        format!("repo-{hash}")
    } else {
        format!("{truncated_slug}-{hash}")
    }
}

/// Lowercase + strip protocol/SSH prefix + trailing `.git` so two URL
/// forms pointing at the same remote produce the same cache id:
///
/// - `https://github.com/owner/repo.git`
/// - `git@github.com:owner/repo.git`
/// - `ssh://git@github.com/owner/repo.git`
/// - `https://github.com/owner/repo`
fn canonicalize_url_for_hashing(url: &str) -> String {
    let lower = url
        .trim()
        .to_ascii_lowercase();
    // Strip scheme prefixes in priority order so the longest matches first.
    let stripped = lower
        .strip_prefix("ssh://git@")
        .or_else(|| lower.strip_prefix("https://"))
        .or_else(|| lower.strip_prefix("http://"))
        .or_else(|| lower.strip_prefix("git@"))
        .unwrap_or(&lower);
    // `git@host:owner/repo` form uses `:` between host and path; treat
    // it as `/` for canonicalization so the slug shape is consistent.
    let stripped = stripped.replace(':', "/");
    // Strip a single trailing `.git` if present.
    let without_dot_git = stripped
        .strip_suffix(".git")
        .unwrap_or(&stripped);
    without_dot_git.to_owned()
}

/// Replace every non-`[a-z0-9-]` char with `-`, collapse runs, trim
/// leading + trailing `-`. Result is always a valid prefix for the
/// `source.id` regex (leading char may be a digit; caller patches if
/// the slug starts with a digit before constructing the final id).
fn sluggify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_was_dash = true; // leading-trim sentinel
    for c in s.chars() {
        let mapped = if c.is_ascii_lowercase() || c.is_ascii_digit() { c } else { '-' };
        if mapped == '-' {
            if last_was_dash {
                continue;
            }
            out.push('-');
            last_was_dash = true;
        } else {
            out.push(mapped);
            last_was_dash = false;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

fn truncate_slug(slug: &str, max_chars: usize) -> String {
    if slug.len() <= max_chars {
        return slug.to_owned();
    }
    let mut truncated: String = slug
        .chars()
        .take(max_chars)
        .collect();
    while truncated.ends_with('-') {
        truncated.pop();
    }
    truncated
}

/// First 8 hex chars of SHA-256(canonical_url). Provides
/// collision-resistance for slugs that survive truncation to the same
/// prefix (e.g. two long owners that share an initial segment).
pub fn url_hash_prefix(canonical_url: &str) -> String {
    let digest = Sha256::digest(canonical_url.as_bytes());
    let mut hex = String::with_capacity(8);
    for byte in &digest[..4] {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_cache_id_for_https_github_url() {
        let id = derive_cache_id("https://github.com/stacks-network/stacks-core.git");
        assert!(
            id.starts_with("github-com-stacks-network-stacks-core-"),
            "unexpected slug shape: {id}",
        );
        assert_eq!(
            id.split('-')
                .next_back()
                .unwrap()
                .len(),
            8,
            "expected 8-char hex suffix: {id}",
        );
    }

    #[test]
    fn derive_cache_id_for_ssh_git_at_form() {
        let id = derive_cache_id("git@github.com:cylewitruk/stacks-core.git");
        assert!(
            id.starts_with("github-com-cylewitruk-stacks-core-"),
            "unexpected slug shape: {id}",
        );
    }

    #[test]
    fn derive_cache_id_distinguishes_forks_with_same_repo_name() {
        let upstream = derive_cache_id("https://github.com/stacks-network/stacks-core.git");
        let fork = derive_cache_id("https://github.com/cylewitruk/stacks-core.git");
        assert_ne!(upstream, fork, "forks with the same repo name must not collide");
    }

    #[test]
    fn derive_cache_id_is_deterministic_across_url_forms_of_same_remote() {
        let https = derive_cache_id("https://github.com/stacks-network/stacks-core.git");
        let https_no_git = derive_cache_id("https://github.com/stacks-network/stacks-core");
        let ssh = derive_cache_id("git@github.com:stacks-network/stacks-core.git");
        let ssh_url = derive_cache_id("ssh://git@github.com/stacks-network/stacks-core.git");
        // All four canonicalize to `github.com/stacks-network/stacks-core`
        // so they SHOULD produce the same id.
        assert_eq!(https, https_no_git);
        assert_eq!(https, ssh);
        assert_eq!(https, ssh_url);
    }

    #[test]
    fn derive_cache_id_always_validates_against_source_id_regex() {
        // Every derived id must round-trip through the validator.
        for url in [
            "https://github.com/stacks-network/stacks-core.git",
            "git@github.com:cylewitruk/stacks-core.git",
            "https://gitlab.example.com/team/very-long-repo-name-that-exceeds-fifty-five-chars.git",
            "https://example.com/_weird_/repo with spaces/_.git",
        ] {
            let id = derive_cache_id(url);
            assert!(
                crate::settings::validate_source_id(&id).is_ok(),
                "derived id `{id}` from `{url}` failed validation: {:?}",
                crate::settings::validate_source_id(&id),
            );
        }
    }

    #[test]
    fn derive_cache_id_caps_total_length_at_64() {
        let very_long = "https://very-long-host.example.com/very-long-owner-name/very-long-repository-name-\
             that-keeps-going-and-going.git";
        let id = derive_cache_id(very_long);
        assert!(id.len() <= 64, "id `{id}` exceeds 64 chars (len={})", id.len());
        assert!(crate::settings::validate_source_id(&id).is_ok());
    }

    #[test]
    fn resolve_cache_id_uses_pinned_when_set() {
        let pinned = resolve_cache_id(Some("my-pinned-id"), "https://example.com/x.git").unwrap();
        assert_eq!(pinned, "my-pinned-id");
    }

    #[test]
    fn resolve_cache_id_derives_when_pinned_unset() {
        let derived =
            resolve_cache_id(None, "https://github.com/stacks-network/stacks-core.git").unwrap();
        assert!(derived.starts_with("github-com-stacks-network-stacks-core-"));
    }

    #[test]
    fn resolve_cache_id_re_validates_pinned_defense_in_depth() {
        // A bad id snuck past settings (constructed at runtime); the
        // resolver MUST still reject it.
        let err = resolve_cache_id(Some("trailing-"), "https://example.com/x.git").unwrap_err();
        assert!(
            err.contains("trailing hyphen") || err.contains("must end with"),
            "expected trailing-hyphen rejection: {err}",
        );
    }
}
