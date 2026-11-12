//! Process-level checks for the credential and output boundaries.

use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

mod mcp;
mod session;

fn command(home: &Path) -> Command {
    let mut command = isolated_process(home);
    command.arg("--config-dir").arg(home.join("config/slk"));
    command
}

fn isolated_process(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_slk"));
    command
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/slk-test-no-session-bus",
        )
        .env("RUST_LOG", "trace")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn run(command: &mut Command, input: &str) -> Output {
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn config_file(home: &Path) -> PathBuf {
    home.join("config/slk/config.json")
}

fn write_profile(home: &Path, kind: &str) -> String {
    let path = config_file(home);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let config = json!({"profiles": {"work": {
        "kind": kind,
        "identity": {"workspace_url": "https://team.slack.com/", "team_id": "T12345678", "user_id": "U87654321"},
        "keyring_ref": "00112233-4455-4677-8899-aabbccddeeff"
    }}}).to_string();
    fs::write(path, &config).unwrap();
    config
}

#[test]
fn help_and_version_work_without_configuration() {
    let home = tempfile::tempdir().unwrap();
    let help = run(command(home.path()).arg("--help"), "");
    assert!(help.status.success());
    assert!(help.stderr.is_empty());
    let text = String::from_utf8(help.stdout).unwrap();
    for name in [
        "auth", "channels", "search", "read", "thread", "send", "reply", "users", "resolve", "dm",
        "edit", "delete", "react", "unreact",
    ] {
        assert!(text.contains(name));
    }
    for name in ["send", "reply", "edit", "delete", "react", "unreact", "dm"] {
        let help = run(command(home.path()).args([name, "--help"]), "");
        assert!(help.status.success());
        assert!(help.stderr.is_empty());
        let text = String::from_utf8(help.stdout).unwrap();
        for flag in ["--dry-run", "--yes", "--profile", "--json"] {
            assert!(text.contains(flag));
        }
        if matches!(name, "send" | "reply" | "edit") {
            assert!(text.contains("--text"));
            assert!(text.contains("--file"));
            assert!(text.contains("--blocks-file"));
        }
    }
    let help = run(command(home.path()).args(["auth", "--help"]), "");
    assert!(help.status.success());
    assert!(help.stderr.is_empty());
    let text = String::from_utf8(help.stdout).unwrap();
    for name in [
        "oauth", "refresh", "browser", "login", "list", "status", "logout",
    ] {
        assert!(text.contains(name));
    }
    let help = run(command(home.path()).args(["auth", "oauth", "--help"]), "");
    assert!(help.status.success());
    let text = String::from_utf8(help.stdout).unwrap();
    for flag in [
        "--client-id",
        "--redirect-uri",
        "--workspace",
        "--scope",
        "--no-open",
    ] {
        assert!(text.contains(flag));
    }
    let help = run(command(home.path()).args(["auth", "browser", "--help"]), "");
    assert!(help.status.success());
    let text = String::from_utf8(help.stdout).unwrap();
    for flag in ["--workspace", "--profile", "--json"] {
        assert!(text.contains(flag));
    }
    let help = run(command(home.path()).args(["users", "--help"]), "");
    assert!(help.status.success());
    let text = String::from_utf8(help.stdout).unwrap();
    for name in ["list", "info", "lookup"] {
        assert!(text.contains(name));
    }
    let version = run(command(home.path()).arg("--version"), "");
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout).unwrap().trim(),
        concat!("slk ", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn piped_stdout_keeps_explicit_output_format_and_does_not_create_config() {
    let home = tempfile::tempdir().unwrap();
    let json_output = run(command(home.path()).args(["auth", "list", "--json"]), "");
    assert!(json_output.status.success());
    assert!(json_output.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&json_output.stdout).unwrap(),
        json!({"data": []})
    );
    let text_output = run(command(home.path()).args(["auth", "list"]), "");
    assert!(text_output.status.success());
    assert_eq!(
        String::from_utf8(text_output.stdout).unwrap().trim(),
        "No profiles configured."
    );
    assert!(!config_file(home.path()).exists());
}

#[test]
fn argument_and_credential_failures_are_single_json_errors_without_secret_echo() {
    let home = tempfile::tempdir().unwrap();
    for (args, input, code) in [
        (vec!["auth", "refresh"], "", "invalid_arguments"),
        (
            vec![
                "auth",
                "oauth",
                "--client-id",
                "1234.5678",
                "--redirect-uri",
                "http://localhost:12345/oauth/callback",
                "--workspace",
                "https://team.slack.com",
                "--scope",
                "chat:write",
            ],
            "",
            "invalid_arguments",
        ),
        (
            vec![
                "auth",
                "oauth",
                "--profile",
                "work",
                "--client-id",
                "1234.5678",
                "--redirect-uri",
                "http://evil.example:12345/oauth/callback",
                "--workspace",
                "https://team.slack.com",
                "--scope",
                "chat:write",
            ],
            "",
            "invalid_arguments",
        ),
        (
            vec![
                "auth",
                "oauth",
                "--profile",
                "work",
                "--client-id",
                "1234.5678",
                "--redirect-uri",
                "http://localhost:12345/oauth/callback",
                "--workspace",
                "https://team.slack.com",
                "--scope",
                "chat:write",
                "--no-open",
            ],
            "",
            "invalid_arguments",
        ),
        (
            vec!["auth", "browser", "--workspace", "https://team.slack.com"],
            "",
            "invalid_arguments",
        ),
        (
            vec!["auth", "browser", "--profile", "work"],
            "",
            "invalid_arguments",
        ),
        (
            vec![
                "auth",
                "browser",
                "--profile",
                "work",
                "--workspace",
                "https://team.slack.com.evil.example",
            ],
            "",
            "invalid_workspace",
        ),
        (
            vec!["auth", "login", "--kind", "user", "--token", "xoxp-test"],
            "",
            "invalid_arguments",
        ),
        (
            vec!["auth", "login", "--kind", "xoxp-test"],
            "",
            "invalid_arguments",
        ),
        (
            vec!["auth", "login", "--kind", "user", "--stdin"],
            r#"{"token":"xoxp-test"}"#,
            "invalid_arguments",
        ),
        (
            vec!["auth", "login", "--kind", "user", "--profile", "work"],
            "",
            "invalid_arguments",
        ),
        (
            vec![
                "auth",
                "login",
                "--kind",
                "user",
                "--profile",
                "work",
                "--stdin",
            ],
            r#"{"token":"xoxb-test"}"#,
            "invalid_credentials",
        ),
        (vec!["auth", "logout"], "", "invalid_arguments"),
    ] {
        let output = run(command(home.path()).args(args).arg("--json"), input);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["code"], code);
        assert!(!String::from_utf8(output.stderr).unwrap().contains("xox"));
    }
    assert!(!config_file(home.path()).exists());
}

#[test]
fn untrusted_browser_origins_fail_before_any_profile_is_saved() {
    let home = tempfile::tempdir().unwrap();
    for workspace in [
        "http://127.0.0.1:1",
        "https://team.slack.com.evil.example",
        "https://user:password@team.slack.com",
        "https://team.slack.com:8443",
    ] {
        let input =
            json!({"workspace_url": workspace, "token": "xoxc-test", "cookie": "xoxd-test"});
        let output = run(
            command(home.path()).args([
                "auth",
                "login",
                "--kind",
                "browser",
                "--profile",
                "work",
                "--stdin",
                "--json",
            ]),
            &input.to_string(),
        );
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stderr).unwrap()["code"],
            "invalid_credentials"
        );
    }
    assert!(!config_file(home.path()).exists());
}

#[test]
fn runtime_errors_leave_stdout_empty_and_corrupt_config_unchanged() {
    let home = tempfile::tempdir().unwrap();
    let no_profiles = run(command(home.path()).args(["auth", "status", "--json"]), "");
    assert_eq!(no_profiles.status.code(), Some(1));
    assert!(no_profiles.stdout.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&no_profiles.stderr).unwrap()["code"],
        "no_profiles"
    );
    let path = config_file(home.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "xoxp-test").unwrap();
    let malformed = run(command(home.path()).args(["auth", "list", "--json"]), "");
    assert_eq!(malformed.status.code(), Some(1));
    assert!(malformed.stdout.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&malformed.stderr).unwrap()["code"],
        "invalid_config"
    );
    assert!(
        !String::from_utf8(malformed.stderr)
            .unwrap()
            .contains("xoxp-test")
    );
    assert_eq!(fs::read_to_string(path).unwrap(), "xoxp-test");
}

#[test]
fn message_arguments_fail_before_credentials_are_loaded() {
    let home = tempfile::tempdir().unwrap();
    let config = write_profile(home.path(), "user");
    for (args, code) in [
        (vec!["read", "#general"], "invalid_arguments"),
        (vec!["users", "info", "C1"], "invalid_arguments"),
        (vec!["users", "lookup", " \t"], "invalid_arguments"),
        (vec!["users", "list", "--limit", "0"], "invalid_arguments"),
        (vec!["resolve", "user", "@"], "invalid_arguments"),
        (vec!["resolve", "channel", "#"], "invalid_arguments"),
        (vec!["dm", "@alice", "--dry-run"], "invalid_arguments"),
        (vec!["channels", "--limit", "0"], "invalid_arguments"),
        (vec!["channels", "--limit", "1000"], "invalid_arguments"),
        (
            vec!["search", "deploy", "--limit", "21"],
            "invalid_arguments",
        ),
        (
            vec!["search", "deploy", "--page", "101"],
            "invalid_arguments",
        ),
        (vec!["search", "deploy", "--page", "1"], "invalid_arguments"),
        (vec!["search", " \t"], "invalid_arguments"),
        (
            vec!["search", "deploy", "--cursor", "next", "--page", "1"],
            "invalid_arguments",
        ),
        (vec!["send", "C1"], "invalid_arguments"),
        (
            vec!["send", "C1", "--text", "body", "--file", "-", "--dry-run"],
            "invalid_arguments",
        ),
        (
            vec!["send", "C1", "--text", "body", "--yes"],
            "invalid_arguments",
        ),
        (
            vec!["send", "C1", "--text", "body", "--profile", "work"],
            "invalid_arguments",
        ),
        (
            vec!["send", "U1", "--text", "body", "--dry-run"],
            "invalid_arguments",
        ),
        (
            vec![
                "thread",
                "http://team.slack.com/archives/C1/p1700000000000001",
            ],
            "invalid_arguments",
        ),
        (
            vec![
                "thread",
                "https://other.slack.com/archives/C1/p1700000000000001",
            ],
            "workspace_mismatch",
        ),
        (
            vec![
                "reply",
                "https://other.slack.com/archives/C1/p1700000000000001",
                "--text",
                "body",
                "--dry-run",
            ],
            "workspace_mismatch",
        ),
        (
            vec![
                "thread",
                "https://team.slack.com/archives/C1/p1700000000000002?thread_ts=1700000000.000001",
                "--thread",
                "1700000000.000003",
            ],
            "invalid_arguments",
        ),
    ] {
        let output = run(command(home.path()).args(&args).arg("--json"), "");
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty());
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["code"], code, "{args:?}");
    }
    let link = "https://team.slack.com/archives/C1/p1700000000000001";
    for mut args in [
        vec!["edit", link],
        vec!["edit", link, "--text", "new", "--file", "-"],
        vec!["react", link, ":wave:"],
        vec!["unreact", link, " "],
    ] {
        args.extend(["--dry-run", "--json"]);
        let output = run(command(home.path()).args(&args), "");
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty());
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stderr).unwrap()["code"],
            "invalid_arguments"
        );
    }
    for args in [
        vec!["edit", link, "--text", "new"],
        vec!["delete", link],
        vec!["react", link, "wave"],
        vec!["unreact", link, "wave"],
        vec!["pin", link],
        vec!["unpin", link],
        vec!["dm", "U1"],
        vec![
            "channel",
            "create",
            "private-room",
            "--visibility",
            "private",
        ],
        vec!["channel", "join", "C1"],
        vec!["channel", "leave", "C1"],
        vec!["channel", "archive", "C1"],
        vec!["channel", "topic", "C1", "New topic"],
        vec!["channel", "mark", "C1", "1700000000.000007"],
        vec!["group-dm", "open", "U1", "U2"],
        vec!["group-dm", "resume", "G1"],
        vec!["group-dm", "close", "G1"],
        vec!["canvas", "append", "F7", "--text", "new"],
        vec!["canvas", "prepend", "F7", "--text", "new"],
        vec![
            "canvas",
            "insert-before",
            "F7",
            "temp:C:one",
            "--text",
            "new",
        ],
        vec![
            "canvas",
            "insert-after",
            "F7",
            "temp:C:one",
            "--text",
            "new",
        ],
        vec![
            "canvas",
            "replace-section",
            "F7",
            "temp:C:one",
            "--text",
            "new",
        ],
        vec!["canvas", "delete-section", "F7", "temp:C:one"],
    ] {
        for approval in [vec![], vec!["--yes"], vec!["--profile", "work"]] {
            for json in [false, true] {
                let mut command = command(home.path());
                command.args(&args).args(&approval);
                if json {
                    command.arg("--json");
                }
                let output = run(&mut command, "");
                assert_eq!(output.status.code(), Some(2), "{args:?}, {approval:?}");
                assert!(output.stdout.is_empty());
                let error = String::from_utf8(output.stderr).unwrap();
                assert!(error.contains("invalid_arguments"));
                assert!(error.contains("--yes and an explicit --profile"));
            }
        }
    }
    assert_eq!(
        fs::read_to_string(config_file(home.path())).unwrap(),
        config
    );
    write_profile(home.path(), "browser");
    let output = run(
        command(home.path()).args(["search", "deploy", "--cursor", "next", "--json"]),
        "",
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stderr).unwrap()["code"],
        "invalid_arguments"
    );
}

#[test]
fn invalid_stdin_and_file_bodies_fail_without_accessing_credentials() {
    let home = tempfile::tempdir().unwrap();
    write_profile(home.path(), "user");
    let file = home.path().join("message.txt");
    for input in ["not JSON", "{}", "[null]"] {
        let output = run(
            command(home.path()).args([
                "send",
                "C1",
                "--text",
                "fallback",
                "--blocks-file",
                "-",
                "--dry-run",
                "--json",
            ]),
            input,
        );
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stderr).unwrap()["code"],
            "invalid_arguments"
        );
    }
    for (body, code, exit_code) in [
        (b" \n\t".to_vec(), "invalid_arguments", 2),
        ("🦀".repeat(4_001).into_bytes(), "message_too_long", 2),
        (vec![0xff], "io_error", 1),
    ] {
        fs::write(&file, body).unwrap();
        for (name, target) in [
            ("send", "C1"),
            (
                "edit",
                "https://team.slack.com/archives/C1/p1700000000000001",
            ),
        ] {
            let output = run(
                command(home.path()).args([
                    name,
                    target,
                    "--file",
                    file.to_str().unwrap(),
                    "--dry-run",
                    "--json",
                ]),
                "",
            );
            assert_eq!(output.status.code(), Some(exit_code));
            assert!(output.stdout.is_empty());
            assert_eq!(
                serde_json::from_slice::<Value>(&output.stderr).unwrap()["code"],
                code
            );
        }
    }
    let output = run(
        command(home.path()).args(["send", "C1", "--file", "-", "--dry-run", "--json"]),
        " \n\t",
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stderr).unwrap()["code"],
        "invalid_arguments"
    );
    for args in [
        vec!["send", "C1", "--text", "body", "--yes"],
        vec!["send", "C1", "--text", "body", "--profile", "work"],
    ] {
        let output = run(command(home.path()).args(args), "");
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .starts_with("invalid_arguments:")
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn unavailable_secret_service_is_an_error_without_plaintext_fallback() {
    let home = tempfile::tempdir().unwrap();
    let path = config_file(home.path());
    let config = write_profile(home.path(), "user");
    let output = run(
        command(home.path()).args(["auth", "status", "--profile", "work", "--json"]),
        "",
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stderr).unwrap()["code"],
        "keyring_unavailable"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), config);
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
}

#[test]
fn browser_launch_failure_cleans_up_without_replacing_profiles() {
    let home = tempfile::tempdir().unwrap();
    let mut config: Value = serde_json::from_str(&write_profile(home.path(), "user")).unwrap();
    config["settings"] = json!({"browser": {"executable": home.path().join("missing-browser")}});
    let config = config.to_string();
    fs::write(config_file(home.path()), &config).unwrap();
    let temporary = home.path().join("temporary");
    fs::create_dir(&temporary).unwrap();
    let output = run(
        command(home.path()).env("TMPDIR", &temporary).args([
            "auth",
            "browser",
            "--profile",
            "browser",
            "--workspace",
            "https://team.slack.com",
            "--json",
        ]),
        "",
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stderr).unwrap()["code"],
        "browser_start_failed"
    );
    assert_eq!(fs::read_dir(&temporary).unwrap().count(), 0);
    assert_eq!(
        fs::read_to_string(config_file(home.path())).unwrap(),
        config
    );
}
