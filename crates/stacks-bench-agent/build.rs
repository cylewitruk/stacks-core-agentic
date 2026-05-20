//! Capture the workspace's HEAD commit SHA at build time and expose it
//! via `env!("SBAGENT_GIT_SHA")`. Used by `sbagent session archive` to
//! record an audit anchor in each `sessions.jsonl` ledger entry — every
//! archived session can be traced back to the exact sbagent binary that
//! produced it.
//!
//! Build outside a git checkout (e.g. from a packaged tarball, or a
//! shallow CI clone with no refs) is supported: `SBAGENT_GIT_SHA` is
//! set to `"unknown"` rather than failing the build. Downstream code
//! treats the literal `"unknown"` as a missing value.

use std::path::Path;
use std::process::Command;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let manifest_dir = Path::new(&manifest_dir);

    let sha = read_head_sha(manifest_dir).unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=SBAGENT_GIT_SHA={sha}");

    // Rebuild whenever HEAD or the resolved-ref file changes. On a
    // normal branch checkout, `.git/HEAD` is a static `ref:
    // refs/heads/<branch>` pointer — watching just it would miss
    // every subsequent commit on that branch. We follow HEAD's
    // pointer and watch the underlying ref file too. Detached HEAD
    // (sha directly in `.git/HEAD`) needs no second watch — every
    // commit rewrites HEAD itself.
    if let Some(git_dir) = find_git_dir(manifest_dir) {
        let head_path = git_dir.join("HEAD");
        println!("cargo:rerun-if-changed={}", head_path.display());
        if let Ok(head_contents) = std::fs::read_to_string(&head_path)
            && let Some(stripped) = head_contents
                .lines()
                .next()
                .and_then(|l| l.strip_prefix("ref: "))
        {
            let ref_path = git_dir.join(stripped.trim());
            // Loose ref file — may not exist if the branch's tip is
            // only in packed-refs.
            if ref_path.is_file() {
                println!("cargo:rerun-if-changed={}", ref_path.display());
            }
        }
        let packed_refs = git_dir.join("packed-refs");
        if packed_refs.exists() {
            println!("cargo:rerun-if-changed={}", packed_refs.display());
        }
    }
}

fn read_head_sha(dir: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8(output.stdout).ok()?;
    let sha = sha.trim();
    if sha.is_empty() {
        return None;
    }
    Some(sha.to_owned())
}

fn find_git_dir(start: &Path) -> Option<std::path::PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        let candidate = dir.join(".git");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if candidate.is_file() {
            // gitlink (worktree); resolve `gitdir: <path>` so rerun-if-changed
            // tracks the real ref store.
            if let Ok(contents) = std::fs::read_to_string(&candidate)
                && let Some(stripped) = contents
                    .lines()
                    .next()
                    .and_then(|l| l.strip_prefix("gitdir: "))
            {
                let resolved = dir.join(stripped.trim());
                if resolved.is_dir() {
                    return Some(resolved);
                }
            }
            return None;
        }
        current = dir.parent();
    }
    None
}
