use super::{command, config_file, run, write_profile};
use rmcp::{ServiceExt, model::CallToolRequestParams, transport::TokioChildProcess};
use serde_json::{Value, json};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;

#[test]
fn mcp_requires_an_existing_explicit_profile_before_starting_stdio() {
    let home = tempfile::tempdir().unwrap();
    write_profile(home.path(), "user");
    for (args, code, exit) in [
        (vec!["mcp"], "invalid_arguments", 2),
        (vec!["mcp", "--profile", "../work"], "invalid_arguments", 2),
        (vec!["mcp", "--profile", "missing"], "profile_not_found", 1),
    ] {
        let output = run(command(home.path()).args(args).arg("--json"), "");
        assert_eq!(output.status.code(), Some(exit));
        assert!(output.stdout.is_empty());
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stderr).unwrap()["code"],
            code
        );
    }
}

#[tokio::test]
async fn mcp_stdio_discovery_help_and_profile_binding_work_without_keyring_access() {
    let home = tempfile::tempdir().unwrap();
    let original = write_profile(home.path(), "user");
    let mut process = tokio::process::Command::from(command(home.path()));
    process.args(["mcp", "--profile", "work"]);
    let (transport, stderr) = TokioChildProcess::builder(process)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let client = ().serve(transport).await.unwrap();
    let info = client.peer_info().unwrap();
    let implementation = info.server_info.as_ref().unwrap();
    assert_eq!(implementation.name, "slk");
    assert_eq!(implementation.version, env!("CARGO_PKG_VERSION"));
    let tools = client.list_tools(None).await.unwrap();
    assert_eq!(
        tools
            .tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>(),
        ["slk_help", "slk_read"]
    );
    for (command, flags) in [
        (vec!["canvas", "sections"], ["--contains", "--section-type"]),
        (vec!["search"], ["--profile", "--workspace"]),
        (vec!["auth", "status"], ["--profile", "--workspace"]),
        (vec!["auth", "whoami"], ["--profile", "--workspace"]),
    ] {
        let help = tokio::time::timeout(
            Duration::from_secs(5),
            client.call_tool(
                CallToolRequestParams::new("slk_help")
                    .with_arguments(json!({"command": command}).as_object().unwrap().clone()),
            ),
        )
        .await
        .expect("the help request must return a response")
        .unwrap();
        assert_ne!(help.is_error, Some(true));
        let text = &help.content[0].as_text().unwrap().text;
        for flag in flags {
            assert!(text.contains(flag), "{command:?}: missing {flag}");
        }
        assert!(!text.contains('\u{1b}'));
    }
    assert_eq!(
        std::fs::read_to_string(config_file(home.path())).unwrap(),
        original
    );

    for field in ["user_id", "team_id", "workspace_url", "kind"] {
        let mut changed: Value = serde_json::from_str(&original).unwrap();
        if field == "kind" {
            changed["profiles"]["work"][field] = json!("browser");
        } else {
            changed["profiles"]["work"]["identity"][field] = match field {
                "user_id" => json!("U00000001"),
                "team_id" => json!("T00000001"),
                _ => json!("https://other.slack.com/"),
            };
        }
        let changed = changed.to_string();
        std::fs::write(config_file(home.path()), &changed).unwrap();
        let result = client
            .call_tool(
                CallToolRequestParams::new("slk_read").with_arguments(
                    json!({"args": ["auth", "status"]})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        assert_eq!(
            result.structured_content.unwrap()["code"],
            "profile_conflict",
            "{field}"
        );
        assert_eq!(
            std::fs::read_to_string(config_file(home.path())).unwrap(),
            changed
        );
    }
    std::fs::write(config_file(home.path()), "{}").unwrap();
    let result = client
        .call_tool(
            CallToolRequestParams::new("slk_read")
                .with_arguments(json!({"args": ["read", "C1"]}).as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        result.structured_content.unwrap()["code"],
        "profile_not_found"
    );
    client.cancel().await.unwrap();
    let mut errors = Vec::new();
    stderr.unwrap().read_to_end(&mut errors).await.unwrap();
    assert!(errors.is_empty(), "{}", String::from_utf8_lossy(&errors));
}
