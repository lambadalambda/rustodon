use std::process::Command;

fn rustodon() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rustodon"))
}

#[test]
fn root_help_lists_process_modes() {
    let output = rustodon()
        .arg("--help")
        .output()
        .expect("rustodon should run");

    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).expect("help should be UTF-8");
    for process_mode in ["web", "worker", "admin", "preflight"] {
        assert!(
            stdout.contains(process_mode),
            "root help should list {process_mode:?}, got:\n{stdout}"
        );
    }
}

#[test]
fn each_process_mode_has_help() {
    for (process_mode, description) in [
        ("web", "Serve HTTP API"),
        ("worker", "Process durable background"),
        ("admin", "Run administrative"),
        ("preflight", "Validate a Mastodon cutover"),
    ] {
        let output = rustodon()
            .args([process_mode, "--help"])
            .output()
            .expect("rustodon should run");

        assert!(
            output.status.success(),
            "{process_mode:?} help failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8(output.stdout).expect("help should be UTF-8");
        assert!(
            stdout.contains("Usage:"),
            "{process_mode:?} should have useful help, got:\n{stdout}"
        );
        assert!(
            stdout.contains(description),
            "{process_mode:?} should explain its purpose, got:\n{stdout}"
        );
    }
}

#[test]
fn preflight_configuration_failure_has_machine_status_and_safe_diagnostic() {
    let output = rustodon()
        .arg("preflight")
        .env_clear()
        .env("SECRET_KEY_BASE", "must-not-appear")
        .output()
        .expect("rustodon preflight should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("error should be UTF-8");
    assert!(stderr.contains("LOCAL_DOMAIN"));
    assert!(!stderr.contains("must-not-appear"));
}

#[test]
fn worker_requires_valid_configuration_without_exposing_secrets() {
    let output = rustodon()
        .arg("worker")
        .env_clear()
        .env("SECRET_KEY_BASE", "must-not-appear")
        .output()
        .expect("rustodon should run");

    assert!(!output.status.success());

    let stderr = String::from_utf8(output.stderr).expect("error should be UTF-8");
    assert!(stderr.contains("LOCAL_DOMAIN"));
    assert!(!stderr.contains("must-not-appear"));
    assert!(!stderr.contains("not implemented yet"));
}

#[test]
fn admin_help_exposes_operational_schema_migration() {
    let output = rustodon()
        .args(["admin", "--help"])
        .output()
        .expect("rustodon admin help should run");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("migrate-operational-schema"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("worker-readiness"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("dead-jobs"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("reset-password"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("create-user"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("resolve-report"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("delete-status"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("reconcile-account-stats"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("suspend-account"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("unsuspend-account"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("block-domain"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("unblock-domain"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("purge-domain"));
}

#[test]
fn migration_help_exposes_writer_role_without_loading_configuration() {
    let output = rustodon()
        .args(["admin", "migrate-operational-schema", "--help"])
        .env_clear()
        .output()
        .expect("migration help should run");
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("--writer-role"));
}

#[test]
fn web_requires_valid_configuration_without_exposing_secrets() {
    let output = rustodon()
        .arg("web")
        .env_clear()
        .env("SECRET_KEY_BASE", "must-not-appear")
        .output()
        .expect("rustodon web should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("error should be UTF-8");
    assert!(stderr.contains("LOCAL_DOMAIN"));
    assert!(!stderr.contains("must-not-appear"));
    assert!(!stderr.contains("not implemented yet"));
}

#[test]
fn remote_refresh_is_an_existing_account_only_operator_command() {
    let output = rustodon()
        .args(["admin", "refresh-remote-account", "--help"])
        .env_clear()
        .output()
        .expect("help runs without configuration");
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("--account-id"));
    assert!(!help.contains("--actor-uri"));
    let output = rustodon()
        .args(["admin", "refresh-remote-account"])
        .env_clear()
        .output()
        .expect("CLI runs");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--account-id"));
}
