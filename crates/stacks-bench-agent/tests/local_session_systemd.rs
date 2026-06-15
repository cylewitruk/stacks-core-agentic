//! Static checks for the local-session systemd operator templates.

const SERVICE: &str =
    include_str!("../../../assets/operator-templates/systemd/sbagent-session.service");
const TIMER: &str =
    include_str!("../../../assets/operator-templates/systemd/sbagent-session.timer");
const ENV_EXAMPLE: &str =
    include_str!("../../../assets/operator-templates/systemd/sbagent-session.env.example");
const OPERATIONS: &str = include_str!("../../../docs/operations.md");

#[test]
fn session_service_runs_session_not_maintain() {
    assert!(SERVICE.contains("[Service]"));
    assert!(SERVICE.contains("Type=oneshot"));
    assert!(SERVICE.contains("User=sbagent"));
    assert!(SERVICE.contains("WorkingDirectory=/srv/stacks-core-autopilot"));
    assert!(SERVICE.contains("EnvironmentFile=/etc/sbagent/sbagent-session.env"));
    assert!(SERVICE.contains("ExecStart=/usr/local/bin/sbagent"));
    assert!(SERVICE.contains("session run"));
    assert!(SERVICE.contains("--publish-accepted-prs"));
    assert!(SERVICE.contains("--archive"));
    assert!(!SERVICE.contains("maintain"), "session service must not invoke or describe maintain",);
    assert!(
        !SERVICE.contains("/bin/sh") && !SERVICE.contains(".sh"),
        "default service must call sbagent directly, without a wrapper script",
    );
}

#[test]
fn session_timer_is_conservative_and_no_catchup() {
    assert!(TIMER.contains("[Timer]"));
    assert!(TIMER.contains("Unit=sbagent-session.service"));
    assert!(TIMER.contains("OnCalendar="));
    assert!(TIMER.contains("OnUnitInactiveSec"));
    assert!(TIMER.contains("Persistent=false"));
    assert!(TIMER.contains("RandomizedDelaySec="));
    assert!(
        !TIMER.contains("sbagent maintain") && !TIMER.contains("sbagent session run"),
        "timer should schedule the service, not embed commands",
    );
}

#[test]
fn env_example_uses_paths_not_secret_values() {
    assert!(ENV_EXAMPLE.contains("SBAGENT_CONFIG=/etc/sbagent/config.toml"));
    assert!(ENV_EXAMPLE.contains("SBAGENT_OPERATOR_DIR=/srv/stacks-core-autopilot"));
    assert!(ENV_EXAMPLE.contains("SBAGENT_TOKEN_FILE=/etc/sbagent/gh_token"));
    assert!(ENV_EXAMPLE.contains("Do not store"));
    assert!(ENV_EXAMPLE.contains("publish.token_file"));

    for forbidden in
        ["github_pat_", "ghp_", "STACKS_BENCH_BOT_PAT", "PAT=", "TOKEN=", "OPENAI_API_KEY"]
    {
        assert!(
            !ENV_EXAMPLE.contains(forbidden),
            "env example must not contain secret-shaped literal `{forbidden}`",
        );
        assert!(
            !SERVICE.contains(forbidden),
            "service must not contain secret-shaped literal `{forbidden}`",
        );
        assert!(
            !TIMER.contains(forbidden),
            "timer must not contain secret-shaped literal `{forbidden}`",
        );
    }
}

#[test]
fn service_exposes_hardening_without_forcing_it() {
    for expected in [
        "# NoNewPrivileges=true",
        "# ProtectSystem=strict",
        "# ProtectHome=true",
        "# PrivateTmp=true",
        "# ReadWritePaths=",
        "ProtectHome=true will block home-directory data/token paths",
    ] {
        assert!(SERVICE.contains(expected), "missing `{expected}`");
    }
}

#[test]
fn operations_docs_pin_install_and_validation_commands() {
    for expected in [
        "systemctl daemon-reload",
        "systemctl enable --now sbagent-session.timer",
        "systemctl list-timers --all sbagent-session.timer",
        "journalctl -u sbagent-session.service",
        "systemd-analyze verify",
        "session run --publish-accepted-prs --archive",
        ".sbagent/pause",
        "GitHub Actions for `sbagent maintain`",
    ] {
        assert!(OPERATIONS.contains(expected), "missing `{expected}`");
    }
}
