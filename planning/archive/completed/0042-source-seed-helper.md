# 0042: Source Seed Helper

- **id:** `0042-source-seed-helper`
- **status:** `shipped`
- **priority:** `medium`
- **iteration:** [v4-v3-polish-and-bot-fork-seed](v4-v3-polish-and-bot-fork-seed.md)

## Shipped

Added `sbagent source seed --from <source-url> --to <dest-url>
[--branch <branch>]` as the replacement for the removed `init --seed-from`
bootstrap path.

## Validation

- Fixture tests cover HTTPS, SSH, and `file://` destination behavior.
- Smoke session `20260611-172955` validated the same bot-fork PAT, branch-push,
  and PR-creation machinery used by the live publishing path.
- Separate seeding against a brand-new GitHub fork was waived before closure.
