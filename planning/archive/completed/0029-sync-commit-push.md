# 0029: Sync Commit / Push Convenience

- **id:** `0029-sync-commit-push`
- **status:** `shipped`
- **completed:** `2026-06-12`

## Problem

Operator bundle sync, committing bundle changes, and pushing them used to be
separate manual steps.

## Shipped

`sbagent sync` now supports:

- `--commit`: stages refreshed `.sbagent/` bundle paths and creates one
  bot-authored commit, skipping clean-tree no-ops.
- `--push`: implies `--commit` and pushes the current branch to `origin` using
  the bot PAT path with URL-prefix validation.
- `--keep-tunables`: preserves operator-edited prompts / context docs when the
  operator intentionally wants to merge bundle changes manually.

## Validation

- `crates/stacks-bench-agent/tests/sync.rs` covers `sync --commit`,
  clean-tree no-op behavior, and push preflight rejection for non-HTTPS origins.
- Live operator use of `sbagent sync --commit --push` successfully committed and
  pushed bundle updates to `stacks-bench-bot/stacks-core-autopilot` during the
  smoke-test cleanup.
