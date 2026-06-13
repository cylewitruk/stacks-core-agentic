//! Static checks for the maintain-only GitHub Actions workflow.

#[test]
fn maintain_workflow_is_maintain_only_and_guarded() {
    let workflow =
        include_str!("../../../assets/operator-templates/.github/workflows/sbagent-maintain.yml");

    assert!(workflow.contains("workflow_dispatch:"));
    assert!(workflow.contains("push:"));
    assert!(workflow.contains("sessions.jsonl"));
    assert!(workflow.contains("cron:"));
    assert!(workflow.contains("group: sbagent-autonomy"));
    assert!(workflow.contains("pull-requests: read"));
    assert!(workflow.contains("contents: write"));
    assert!(workflow.contains("github.actor != 'stacks-bench-bot'"));
    assert!(
        !workflow.contains("github.repository =="),
        "operator template must not be hard-coded to one repository",
    );
    assert!(workflow.contains("secrets.SBAGENT_CONFIG_TOML != ''"));
    assert!(workflow.contains("secrets.STACKS_BENCH_BOT_PAT != ''"));
    assert!(workflow.contains("--package stacks-bench-agent --bin sbagent"));
    assert!(workflow.contains("STACKS_BENCH_BOT_PAT"));
    assert!(workflow.contains("$HOME/.config/sbagent/gh_token"));
    assert!(workflow.contains("chmod 600"));
    assert!(workflow.contains("sbagent maintain"));
    assert!(
        !workflow.contains("sbagent session run"),
        "maintain workflow must never start benchmark sessions",
    );
}
