//! Shared primitive for the per-phase `clean` subcommands.
//!
//! Each phase that produces session artifacts can be re-run from scratch
//! by removing the files and directories it owns. The primitive here
//! takes a list of paths and removes whichever exist; missing paths are
//! silently skipped so cleans are idempotent.

use std::path::Path;

use anyhow::{Context as _, Result};

/// Stats reported by [`remove_paths`]. Useful for one-line summaries
/// like `"removed 3 file(s), 1 dir(s); 2 missing"`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CleanReport {
    /// Files removed (regular files, symlinks).
    pub removed_files: usize,
    /// Directories removed (recursively).
    pub removed_dirs: usize,
    /// Paths that didn't exist when checked. Counted, not surfaced.
    pub skipped_missing: usize,
}

impl CleanReport {
    /// Total entries that were actually removed.
    pub fn total_removed(&self) -> usize {
        self.removed_files + self.removed_dirs
    }

    /// Merge another report into this one. Used when a phase clean
    /// composes multiple sub-removals (e.g. `optimize clean` drops the
    /// `optimize/` tree AND tears down git worktrees).
    pub fn merge(&mut self, other: CleanReport) {
        self.removed_files += other.removed_files;
        self.removed_dirs += other.removed_dirs;
        self.skipped_missing += other.skipped_missing;
    }
}

/// Remove every path in `paths` that exists. Files are removed via
/// [`std::fs::remove_file`]; directories via [`std::fs::remove_dir_all`].
/// Missing paths are silently skipped (recorded in
/// [`CleanReport::skipped_missing`]).
///
/// Errors short-circuit and surface the failing path. Callers that want
/// best-effort semantics can iterate themselves and call
/// [`remove_one`] per path.
pub fn remove_paths<I, P>(paths: I) -> Result<CleanReport>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let mut report = CleanReport::default();
    for p in paths {
        report.merge(remove_one(p.as_ref())?);
    }
    Ok(report)
}

/// Remove a single path. Public so phase cleans that need to interleave
/// extra work (e.g. `git worktree remove` before deleting the dir) can
/// use the same accounting.
pub fn remove_one(path: &Path) -> Result<CleanReport> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.is_dir() {
                std::fs::remove_dir_all(path)
                    .with_context(|| format!("removing dir {}", path.display()))?;
                Ok(CleanReport {
                    removed_files: 0,
                    removed_dirs: 1,
                    skipped_missing: 0,
                })
            } else {
                std::fs::remove_file(path)
                    .with_context(|| format!("removing file {}", path.display()))?;
                Ok(CleanReport {
                    removed_files: 1,
                    removed_dirs: 0,
                    skipped_missing: 0,
                })
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(CleanReport {
            removed_files: 0,
            removed_dirs: 0,
            skipped_missing: 1,
        }),
        Err(e) => Err(anyhow::Error::new(e).context(format!("statting {}", path.display()))),
    }
}

/// Pretty-print a report on a single line. Used by every phase clean's
/// terminal output so operators see a uniform shape.
pub fn print_report(prefix: &str, report: &CleanReport) {
    println!(
        "{prefix}: removed {} file(s), {} dir(s); {} missing",
        report.removed_files, report.removed_dirs, report.skipped_missing,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_paths_handles_files_dirs_and_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("a.txt");
        let dir = tmp.path().join("subdir");
        let nested = dir.join("nested.txt");
        let missing = tmp
            .path()
            .join("never-existed");
        std::fs::write(&file, "x").unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&nested, "y").unwrap();

        let report = remove_paths([&file, &dir, &missing]).unwrap();
        assert_eq!(report.removed_files, 1);
        assert_eq!(report.removed_dirs, 1);
        assert_eq!(report.skipped_missing, 1);
        assert!(!file.exists());
        assert!(!dir.exists());
        assert!(!nested.exists());
    }

    #[test]
    fn remove_paths_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("a.txt");
        std::fs::write(&file, "x").unwrap();

        let r1 = remove_paths([&file]).unwrap();
        let r2 = remove_paths([&file]).unwrap();
        assert_eq!(r1.removed_files, 1);
        assert_eq!(r2.skipped_missing, 1);
        assert_eq!(r2.removed_files, 0);
    }

    #[test]
    fn report_merge_sums_components() {
        let mut a = CleanReport {
            removed_files: 1,
            removed_dirs: 2,
            skipped_missing: 3,
        };
        let b = CleanReport {
            removed_files: 4,
            removed_dirs: 5,
            skipped_missing: 6,
        };
        a.merge(b);
        assert_eq!(
            a,
            CleanReport {
                removed_files: 5,
                removed_dirs: 7,
                skipped_missing: 9
            }
        );
    }
}
