//! `sbagent session tail` — multiplex tail of every JSONL/stderr log
//! produced under `<session>/results`.
//!
//! Implementation: poll-based reader. Every cycle we rescan the session
//! tree for new triage / analyzer / optimizer / bench files, then read any
//! bytes appended since the last cycle and print them prefixed with the
//! file's path. Polling lets us pick up files created mid-session — a
//! `tail -F <glob>` approach can't follow paths that don't yet exist when
//! the command starts.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use clap::Args;

use crate::cli::CliContext;
use crate::session::SessionLayout;
use crate::types::SessionId;

const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Args for `sbagent session tail`.
#[derive(Debug, Args)]
pub struct TailSessionArgs {}

/// Multiplex tail of every JSONL/stderr stream in the session.
pub async fn run(_args: TailSessionArgs, ctx: &CliContext, session_id: &SessionId) -> Result<()> {
    let layout = SessionLayout::from_layout(&ctx.layout, session_id.clone());
    if !layout.results_dir.is_dir() {
        bail!("session {} has no results dir at {}", session_id, layout.results_dir.display());
    }

    // Pre-seed offsets for files that already exist at startup. They
    // anchor at end-of-file so we don't replay historical logs (matches
    // `tail -F`'s default for already-existing files).
    //
    // Files that DON'T yet exist at startup are intentionally NOT in the
    // map. When they later appear, `drain_one` treats them as new and
    // streams from byte 0 — also matching `tail -F missing`'s behavior of
    // printing the file's contents from the start once it appears.
    let mut offsets: HashMap<PathBuf, u64> = HashMap::new();
    for path in collect_tail_targets(&layout) {
        if let Ok(meta) = std::fs::metadata(&path)
            && meta.is_file()
        {
            offsets.insert(path, meta.len());
        }
    }
    let prefix_strip = layout.results_dir.clone();

    loop {
        for path in collect_tail_targets(&layout) {
            if let Err(e) = drain_one(&path, &prefix_strip, &mut offsets) {
                tracing::debug!(path = %path.display(), error = %e, "tail drain failed");
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Read any bytes appended to `path` since `offsets[path]` and print them
/// line-by-line, each line prefixed with the path relative to
/// `<session>/results`.
fn drain_one(
    path: &std::path::Path,
    relative_to: &std::path::Path,
    offsets: &mut HashMap<PathBuf, u64>,
) -> Result<()> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return Ok(()), // not yet created — try again next cycle
    };
    if !meta.is_file() {
        return Ok(());
    }
    let len = meta.len();

    // Two distinct first-encounter cases, dispatched on whether the run()
    // pre-seeding step saw the file at startup:
    //
    // - Path is already in `offsets` → file existed at startup, we anchored at
    //   end-of-file. Don't replay history.
    // - Path NOT in `offsets` → file appeared mid-session. Stream from byte 0 so
    //   the operator sees the full output (matches `tail -F missing` semantics).
    let mut offset = *offsets
        .entry(path.to_path_buf())
        .or_insert(0);

    // Truncation: cached offset exceeds current end (e.g. a phase rotated
    // its log). Reset to 0 and resume from new content's start.
    if offset > len {
        offset = 0;
        offsets.insert(path.to_path_buf(), 0);
    }
    if offset == len {
        return Ok(());
    }

    let mut file =
        std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    file.seek(SeekFrom::Start(offset))
        .with_context(|| format!("seeking {} to {offset}", path.display()))?;
    let mut buf = Vec::with_capacity((len - offset) as usize);
    file.read_to_end(&mut buf)
        .with_context(|| format!("reading {}", path.display()))?;

    // Only print and advance past complete lines. Bytes after the last
    // newline are a partial line — leave them buffered so the next cycle
    // can finish the line instead of splitting it.
    let printable_end = match buf
        .iter()
        .rposition(|&b| b == b'\n')
    {
        Some(i) => i + 1,
        None => return Ok(()), // no complete line yet
    };
    let display = path
        .strip_prefix(relative_to)
        .unwrap_or(path)
        .display()
        .to_string();
    let s = String::from_utf8_lossy(&buf[..printable_end]);
    for line in s.lines() {
        println!("[{display}] {line}");
    }
    offsets.insert(path.to_path_buf(), offset + printable_end as u64);
    Ok(())
}

/// Build the path set for the current session tree. Includes paths that
/// don't yet exist — they'll start streaming once the producing phase
/// creates them.
fn collect_tail_targets(layout: &SessionLayout) -> Vec<PathBuf> {
    let mut out = vec![
        layout.triage_stderr(),
        layout.triage_events(),
        layout.merge_stderr(),
        layout.merge_events(),
    ];
    if let Ok(rd) = std::fs::read_dir(layout.analysis_dir()) {
        for entry in rd.flatten() {
            if !entry
                .file_type()
                .map(|t| t.is_dir())
                .unwrap_or(false)
            {
                continue;
            }
            let dir = entry.path();
            out.push(dir.join("stderr.log"));
            out.push(dir.join("events.jsonl"));
        }
    }
    if let Ok(rd) = std::fs::read_dir(layout.optimize_dir()) {
        for entry in rd.flatten() {
            if !entry
                .file_type()
                .map(|t| t.is_dir())
                .unwrap_or(false)
            {
                continue;
            }
            let dir = entry.path();
            out.push(dir.join("stderr.log"));
            out.push(dir.join("events.jsonl"));
            out.push(dir.join("cargo-build.stderr.log"));
            if let Ok(srd) = std::fs::read_dir(&dir) {
                for sub in srd.flatten() {
                    if !sub
                        .file_type()
                        .map(|t| t.is_dir())
                        .unwrap_or(false)
                    {
                        continue;
                    }
                    let name = sub.file_name();
                    if name
                        .to_string_lossy()
                        .starts_with("run-")
                    {
                        out.push(
                            sub.path()
                                .join("bench-run.stderr.log"),
                        );
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionLayout;
    use crate::types::SessionId;

    #[test]
    fn collect_tail_targets_walks_session_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let id: SessionId = "20260507-104400"
            .to_owned()
            .try_into()
            .unwrap();
        let layout = SessionLayout::new(tmp.path(), id);

        std::fs::create_dir_all(&layout.results_dir).unwrap();
        std::fs::create_dir_all(layout.analysis_family_dir("fam-a")).unwrap();
        std::fs::create_dir_all(
            layout
                .experiment_dir("target-x")
                .join("run-1"),
        )
        .unwrap();

        let targets = collect_tail_targets(&layout);
        assert!(
            targets
                .iter()
                .any(|p| p.ends_with("triage/events.jsonl"))
        );
        assert!(
            targets
                .iter()
                .any(|p| p.ends_with("analysis/fam-a/events.jsonl"))
        );
        assert!(
            targets
                .iter()
                .any(|p| p.ends_with("optimize/target-x/run-1/bench-run.stderr.log")),
            "optimize/target-x/run-1/bench-run.stderr.log should be tailed; got {targets:?}",
        );
    }

    #[test]
    fn drain_one_streams_late_arriving_files_from_byte_zero() {
        // Models the `run()` flow where a file did NOT exist at startup
        // (so the pre-seeding step never inserted it into `offsets`),
        // then a phase creates it with several lines before our next
        // poll. We must replay the full content — anything else loses
        // the early output.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp
            .path()
            .join("triage-events.jsonl");
        let mut offsets = HashMap::new();

        // File appears with three lines worth of content already buffered.
        std::fs::write(&path, "alpha\nbeta\ngamma\n").unwrap();
        drain_one(&path, tmp.path(), &mut offsets).unwrap();
        assert_eq!(
            offsets.get(&path).copied(),
            Some(17),
            "newly-appearing file must replay from byte 0, not anchor at end"
        );
    }

    #[test]
    fn drain_one_does_not_replay_history_for_pre_seeded_files() {
        // Models the `run()` flow where the file existed at startup —
        // pre-seeding inserted offset=len, so subsequent appends stream
        // but pre-existing bytes are NOT replayed.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp
            .path()
            .join("triage-events.jsonl");
        std::fs::write(&path, "history\n").unwrap();
        let mut offsets = HashMap::new();
        // Pre-seed (mirrors what run() does at startup).
        offsets.insert(
            path.clone(),
            std::fs::metadata(&path)
                .unwrap()
                .len(),
        );

        // No appends yet → nothing changes.
        drain_one(&path, tmp.path(), &mut offsets).unwrap();
        assert_eq!(offsets.get(&path).copied(), Some(8));

        // Append — only the new line is consumed.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        std::io::Write::write_all(&mut f, b"now\n").unwrap();
        drain_one(&path, tmp.path(), &mut offsets).unwrap();
        assert_eq!(offsets.get(&path).copied(), Some(12));
    }

    #[test]
    fn drain_one_keeps_partial_trailing_line_for_next_cycle() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp
            .path()
            .join("merge-stderr.log");
        std::fs::write(&path, "").unwrap();
        let mut offsets = HashMap::new();
        // Anchor at 0.
        drain_one(&path, tmp.path(), &mut offsets).unwrap();

        // Write one complete line + a partial. Offset must advance only
        // past the newline (12), leaving "PART" buffered in the file.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        std::io::Write::write_all(&mut f, b"line-one-ok\nPART").unwrap();
        drain_one(&path, tmp.path(), &mut offsets).unwrap();
        assert_eq!(
            offsets.get(&path).copied(),
            Some(12),
            "should stop at the last newline, leaving the partial line"
        );

        // Finish the partial line; the resumed cycle prints it and
        // advances all the way.
        std::io::Write::write_all(&mut f, b"IAL-rest\n").unwrap();
        drain_one(&path, tmp.path(), &mut offsets).unwrap();
        assert_eq!(offsets.get(&path).copied(), Some(25));
    }

    #[test]
    fn drain_one_resets_offset_when_file_is_truncated() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp
            .path()
            .join("subagent-stderr.log");
        std::fs::write(&path, "old-and-long-content\n").unwrap();
        let mut offsets = HashMap::new();
        // Pre-seed so the first drain doesn't replay history; we want to
        // exercise the truncation reset, not the late-arrival path.
        offsets.insert(
            path.clone(),
            std::fs::metadata(&path)
                .unwrap()
                .len(),
        );
        drain_one(&path, tmp.path(), &mut offsets).unwrap();
        assert_eq!(offsets.get(&path).copied(), Some(21));

        // Truncate (file gets rotated to a much shorter one).
        std::fs::write(&path, "fresh\n").unwrap();
        drain_one(&path, tmp.path(), &mut offsets).unwrap();
        // Should have noticed truncation and read from start. New length is 6.
        assert_eq!(offsets.get(&path).copied(), Some(6));
    }

    #[test]
    fn drain_one_skips_missing_files_silently() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp
            .path()
            .join("does-not-exist");
        let mut offsets = HashMap::new();
        drain_one(&path, tmp.path(), &mut offsets).unwrap();
        assert!(offsets.is_empty());
    }
}
