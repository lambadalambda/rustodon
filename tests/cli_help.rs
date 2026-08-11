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
fn unimplemented_process_modes_fail_loudly() {
    for process_mode in ["web", "worker", "admin"] {
        let output = rustodon()
            .arg(process_mode)
            .output()
            .expect("rustodon should run");

        assert!(
            !output.status.success(),
            "{process_mode:?} should fail until it is implemented"
        );

        let stderr = String::from_utf8(output.stderr).expect("error should be UTF-8");
        assert!(
            stderr.contains("not implemented yet"),
            "{process_mode:?} should explain why it failed, got:\n{stderr}"
        );
    }
}
