use assert_cmd::Command;
use serde_json::Value;
use tempfile::tempdir;

#[test]
fn json_errors_are_machine_readable_when_token_is_missing() {
    let home = tempdir().unwrap();
    let mut cmd = Command::cargo_bin("dairo").unwrap();
    let assert = cmd
        .env_remove("DAIRO_API_KEY")
        .env_remove("DAIRO_API_URL")
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .args(["--json", "domain", "list"])
        .assert()
        .failure();

    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    let payload: Value = serde_json::from_str(&stderr).expect("stderr should be JSON");
    assert_eq!(payload["error"]["code"], "command_failed");
    assert!(payload["error"]["message"]
        .as_str()
        .unwrap()
        .contains("missing Dairo API token"));
}

#[test]
fn token_set_rejects_positional_token_to_avoid_process_history_leaks() {
    let mut cmd = Command::cargo_bin("dairo").unwrap();
    let assert = cmd
        .args(["auth", "token", "set", "dairo_test_secret"])
        .assert()
        .failure();

    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(stderr.contains("token must be provided on stdin"));
    assert!(
        !stderr.contains("dairo_test_secret"),
        "rejected token value must not be echoed to stderr"
    );
}

#[test]
fn json_parse_errors_are_machine_readable() {
    let mut cmd = Command::cargo_bin("dairo").unwrap();
    let assert = cmd
        .args([
            "--json",
            "send",
            "--inbox-id",
            "inbox_123",
            "--text",
            "Body",
        ])
        .assert()
        .failure();

    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    let payload: Value = serde_json::from_str(&stderr).expect("stderr should be JSON");
    assert_eq!(payload["error"]["code"], "usage_error");
    assert!(payload["error"]["message"]
        .as_str()
        .unwrap()
        .contains("--to"));
}

#[test]
fn json_token_set_reject_does_not_echo_secret() {
    let mut cmd = Command::cargo_bin("dairo").unwrap();
    let assert = cmd
        .args(["--json", "auth", "token", "set", "dairo_test_secret"])
        .assert()
        .failure();

    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    let payload: Value = serde_json::from_str(&stderr).expect("stderr should be JSON");
    assert_eq!(payload["error"]["code"], "usage_error");
    assert!(payload["error"]["message"]
        .as_str()
        .unwrap()
        .contains("token must be provided on stdin"));
    assert!(
        !stderr.contains("dairo_test_secret"),
        "rejected token value must not be echoed in JSON stderr"
    );
}

#[test]
fn dash_prefixed_token_reject_does_not_echo_secret() {
    let mut cmd = Command::cargo_bin("dairo").unwrap();
    let assert = cmd
        .args(["--json", "auth", "token", "set", "--dairo_test_secret"])
        .assert()
        .failure();

    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    let payload: Value = serde_json::from_str(&stderr).expect("stderr should be JSON");
    assert_eq!(payload["error"]["code"], "usage_error");
    assert!(payload["error"]["message"]
        .as_str()
        .unwrap()
        .contains("token must be provided on stdin"));
    assert!(
        !stderr.contains("dairo_test_secret"),
        "dash-prefixed rejected token value must not be echoed in stderr"
    );
}

#[test]
fn malformed_json_flag_still_uses_json_error_envelope() {
    let mut cmd = Command::cargo_bin("dairo").unwrap();
    let assert = cmd.args(["--json=1", "domain", "list"]).assert().failure();

    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    let payload: Value = serde_json::from_str(&stderr).expect("stderr should be JSON");
    assert_eq!(payload["error"]["code"], "usage_error");
    assert!(payload["error"]["message"]
        .as_str()
        .unwrap()
        .contains("--json"));
}

#[test]
fn version_exits_successfully() {
    let mut cmd = Command::cargo_bin("dairo").unwrap();
    cmd.arg("--version").assert().success();
}

#[test]
fn json_version_preserves_normal_version_output() {
    let mut cmd = Command::cargo_bin("dairo").unwrap();
    let assert = cmd.args(["--json", "--version"]).assert().success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(stdout.contains("dairo "));
    assert!(
        stderr.is_empty(),
        "version output must not be wrapped as a JSON error"
    );
}

#[test]
fn json_empty_token_stdin_error_is_clean_json() {
    let mut cmd = Command::cargo_bin("dairo").unwrap();
    let assert = cmd
        .args(["--json", "auth", "token", "set"])
        .write_stdin("")
        .assert()
        .failure();

    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    let payload: Value = serde_json::from_str(&stderr).expect("stderr should be JSON only");
    assert_eq!(payload["error"]["code"], "command_failed");
    assert!(payload["error"]["message"]
        .as_str()
        .unwrap()
        .contains("token cannot be empty"));
}
