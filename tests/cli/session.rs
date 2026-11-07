use super::{config_file, isolated_process, run, write_profile};
use rmcp::{ServiceExt, model::CallToolRequestParams, transport::TokioChildProcess};
use serde_json::{Value, json};
use std::fs;
use std::process::Stdio;
use tokio::io::AsyncReadExt;

fn session(kind: &str, gov: bool) -> Value {
    let workspace = if gov {
        "https://agency.slack-gov.com/"
    } else {
        "https://team.slack.com/"
    };
    let mut input = json!({
        "kind": kind,
        "identity": {"workspace_url": workspace, "team_id": "T12345678", "user_id": "U87654321"},
        "credentials": {"token": "xoxp-test"},
        "settings": {"page_size": 9, "max_pages": 3}
    });
    match kind {
        "bot" => {
            input["identity"]["bot_id"] = json!("B7");
            input["credentials"]["token"] = json!("xoxb-test");
        }
        "browser" => {
            input["credentials"] =
                json!({"token": "xoxc-test", "cookie": "xoxd-test", "workspace_url": workspace});
        }
        "user" => (),
        _ => unreachable!(),
    }
    if gov {
        input["credentials"]["realm"] = json!("gov");
    }
    input
}

#[test]
fn injected_sessions_do_not_load_keyrings_or_persist_secrets() {
    let home = tempfile::tempdir().unwrap();
    write_profile(home.path(), "user");
    fs::write(config_file(home.path()), "corrupt persistent config").unwrap();
    for kind in ["user", "browser", "bot"] {
        for gov in [false, true] {
            let input = session(kind, gov);
            let encoded = input.to_string();
            for source in ["--session-stdin", "--session-env"] {
                let mut command = isolated_process(home.path());
                command.args(["--profile", "injected", source]);
                if source == "--session-env" {
                    command
                        .arg("SLK_TEST_SESSION")
                        .env("SLK_TEST_SESSION", &encoded);
                }
                command.args(["auth", "list", "--json"]);
                let output = run(
                    &mut command,
                    if source == "--session-stdin" {
                        &encoded
                    } else {
                        ""
                    },
                );
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(output.stderr.is_empty());
                let value: Value = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(value["data"].as_array().unwrap().len(), 1);
                assert_eq!(value["data"][0]["profile"], "injected");
                assert_eq!(value["data"][0]["kind"], kind);
                assert_eq!(
                    value["data"][0]["workspace_url"],
                    input["identity"]["workspace_url"]
                );
                assert_eq!(value["data"][0]["bot_id"], input["identity"]["bot_id"]);
                assert!(!value.to_string().contains("xox"));
                assert_eq!(
                    fs::read_to_string(config_file(home.path())).unwrap(),
                    "corrupt persistent config"
                );
                assert_eq!(
                    fs::read_dir(config_file(home.path()).parent().unwrap())
                        .unwrap()
                        .count(),
                    1
                );
            }
        }
    }
}

#[test]
fn injected_sessions_reject_identity_mismatches_and_authentication_mutations() {
    let home = tempfile::tempdir().unwrap();
    let original = write_profile(home.path(), "user");
    let valid = session("user", false);
    let mut wrong_realm = valid.clone();
    wrong_realm["credentials"]["realm"] = json!("gov");
    let mut wrong_bot = session("bot", false);
    wrong_bot["identity"]
        .as_object_mut()
        .unwrap()
        .remove("bot_id");
    let mut wrong_browser = session("browser", false);
    wrong_browser["credentials"]["workspace_url"] = json!("https://other.slack.com");
    let mut unknown = valid.clone();
    unknown["extra"] = json!("xoxp-do-not-echo");
    for (input, code) in [
        (wrong_realm, "identity_mismatch"),
        (wrong_bot, "identity_mismatch"),
        (wrong_browser, "identity_mismatch"),
        (unknown, "invalid_credentials"),
    ] {
        let output = run(
            isolated_process(home.path())
                .env("SLK_TEST_SESSION", input.to_string())
                .args([
                    "--profile",
                    "work",
                    "--session-env",
                    "SLK_TEST_SESSION",
                    "auth",
                    "list",
                    "--json",
                ]),
            "",
        );
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["code"], code);
        assert!(!error.to_string().contains("xox"));
    }
    for args in [
        vec!["auth", "logout"],
        vec!["auth", "refresh"],
        vec!["auth", "login", "--kind", "user", "--stdin"],
        vec!["auth", "browser", "--workspace", "https://team.slack.com"],
    ] {
        let output = run(
            isolated_process(home.path())
                .env("SLK_TEST_SESSION", valid.to_string())
                .args([
                    "--profile",
                    "work",
                    "--session-env",
                    "SLK_TEST_SESSION",
                    "--json",
                ])
                .args(args),
            "",
        );
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stderr).unwrap()["code"],
            "invalid_arguments"
        );
    }
    let output = run(
        isolated_process(home.path())
            .env_remove("SLK_TEST_SESSION")
            .args([
                "--profile",
                "work",
                "--session-env",
                "SLK_TEST_SESSION",
                "auth",
                "list",
                "--json",
            ]),
        "",
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stderr).unwrap()["code"],
        "invalid_credentials"
    );
    for input in [
        session("bot", false),
        session("bot", true),
        session("user", true),
    ] {
        let output = run(
            isolated_process(home.path())
                .env("SLK_TEST_SESSION", input.to_string())
                .args([
                    "--profile",
                    "work",
                    "--session-env",
                    "SLK_TEST_SESSION",
                    "search",
                    "query",
                    "--json",
                ]),
            "",
        );
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stderr).unwrap()["code"],
            "unsupported_capability"
        );
    }
    assert_eq!(
        fs::read_to_string(config_file(home.path())).unwrap(),
        original
    );
    assert_eq!(
        fs::read_dir(config_file(home.path()).parent().unwrap())
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn session_stdin_conflicts_fail_before_consuming_input_and_without_fallback() {
    let home = tempfile::tempdir().unwrap();
    for args in [
        vec!["auth", "list", "--session-stdin"],
        vec!["auth", "list", "--session-env", "SLK_TEST_SESSION"],
        vec![
            "--profile",
            "work",
            "--session-stdin",
            "--session-env",
            "SLK_TEST_SESSION",
            "auth",
            "list",
        ],
        vec![
            "--profile",
            "work",
            "--session-stdin",
            "--config-dir",
            "unused",
            "auth",
            "list",
        ],
        vec!["--profile", "work", "--session-stdin", "mcp"],
        vec![
            "--profile",
            "work",
            "--session-stdin",
            "auth",
            "login",
            "--kind",
            "user",
            "--stdin",
        ],
        vec![
            "--profile",
            "work",
            "--session-stdin",
            "send",
            "C1",
            "--file",
            "-",
            "--dry-run",
        ],
        vec![
            "--profile",
            "work",
            "--session-stdin",
            "send",
            "C1",
            "--text",
            "body",
            "--blocks-file",
            "-",
            "--dry-run",
        ],
        vec![
            "--profile",
            "work",
            "--session-stdin",
            "messages",
            "send",
            "--recipient-id",
            "C1",
            "--message-file",
            "-",
            "--dry-run",
        ],
    ] {
        let output = run(
            isolated_process(home.path()).args(&args).arg("--json"),
            "invalid session that must not be parsed",
        );
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty());
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stderr).unwrap()["code"],
            "invalid_arguments",
            "{args:?}"
        );
    }
    assert_eq!(fs::read_dir(home.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn environment_injected_mcp_keeps_the_bound_session_without_storage_access() {
    let home = tempfile::tempdir().unwrap();
    let mut process = tokio::process::Command::from(isolated_process(home.path()));
    process
        .env("SLK_TEST_SESSION", session("bot", false).to_string())
        .args([
            "--profile",
            "automation",
            "--session-env",
            "SLK_TEST_SESSION",
            "mcp",
        ]);
    let (transport, stderr) = TokioChildProcess::builder(process)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let client = ().serve(transport).await.unwrap();
    assert_eq!(client.list_tools(None).await.unwrap().tools.len(), 2);
    for (args, code) in [
        (vec!["search", "literal"], "unsupported_capability"),
        (
            vec!["conversations", "list", "--workspace", "automation"],
            "invalid_arguments",
        ),
        (
            vec![
                "read",
                "C1",
                "--session-env",
                "SLK_TEST_SESSION",
                "--profile",
                "automation",
            ],
            "invalid_arguments",
        ),
    ] {
        let result = client
            .call_tool(
                CallToolRequestParams::new("slk_read")
                    .with_arguments(json!({"args": args}).as_object().unwrap().clone()),
            )
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        assert_eq!(result.structured_content.unwrap()["code"], code);
    }
    client.cancel().await.unwrap();
    let mut errors = Vec::new();
    stderr.unwrap().read_to_end(&mut errors).await.unwrap();
    assert!(errors.is_empty());
    assert_eq!(fs::read_dir(home.path()).unwrap().count(), 0);
}
