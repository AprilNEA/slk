use super::{command, config_file, run, write_profile};
use rmcp::{
    ServiceExt,
    model::CallToolRequestParams,
    transport::{
        StreamableHttpClientTransport, TokioChildProcess,
        streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::{Value, json};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

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

#[tokio::test]
async fn mcp_http_requires_explicit_authentication_hosts_and_valid_options() {
    let home = tempfile::tempdir().unwrap();
    write_profile(home.path(), "user");
    let http = [
        "--listen",
        "127.0.0.1:0",
        "--bearer-token-env",
        "SLK_TEST_MCP_TOKEN",
        "--allowed-host",
        "127.0.0.1",
    ];
    for (args, token) in [
        (vec!["--allow-writes"], None),
        (vec!["--listen", "127.0.0.1:0"], None),
        (http[..4].to_vec(), Some("synthetic-mcp-token")),
        (http[2..].to_vec(), Some("synthetic-mcp-token")),
        (http.to_vec(), None),
        (http.to_vec(), Some("")),
        (http.to_vec(), Some("synthetic token")),
        (http.to_vec(), Some("synthetic\r\nheader")),
        (
            [http.as_slice(), &["--max-request-bytes", "0"]].concat(),
            Some("synthetic-mcp-token"),
        ),
        (
            [
                http.as_slice(),
                &["--allowed-host", "https://example.com/path"],
            ]
            .concat(),
            Some("synthetic-mcp-token"),
        ),
    ] {
        let mut process = tokio::process::Command::from(command(home.path()));
        process
            .args(["mcp", "--profile", "work", "--json"])
            .args(&args)
            .env_remove("SLK_TEST_MCP_TOKEN")
            .stdin(Stdio::null())
            .kill_on_drop(true);
        if let Some(token) = token {
            process.env("SLK_TEST_MCP_TOKEN", token);
        }
        let output = tokio::time::timeout(Duration::from_secs(5), process.output())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty());
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["code"], "invalid_arguments");
        assert!(!error.to_string().contains("synthetic"));
    }
}

#[tokio::test]
async fn mcp_http_process_preserves_write_approval_metadata_and_identity_binding() {
    match rustls::crypto::ring::default_provider().install_default() {
        Ok(()) | Err(_) => (),
    }
    let home = tempfile::tempdir().unwrap();
    let original = write_profile(home.path(), "user");
    let token = "synthetic-process-mcp-token";
    let mut process = tokio::process::Command::from(command(home.path()))
        .args([
            "mcp",
            "--profile",
            "work",
            "--listen",
            "127.0.0.1:0",
            "--bearer-token-env",
            "SLK_TEST_MCP_TOKEN",
            "--allowed-host",
            "slk.example.test:443",
            "--allow-writes",
        ])
        .env("SLK_TEST_MCP_TOKEN", token)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(process.stderr.take().unwrap());
    let mut ready = String::new();
    tokio::time::timeout(Duration::from_secs(5), stderr.read_line(&mut ready))
        .await
        .unwrap()
        .unwrap();
    let url = format!(
        "http://{}",
        ready.trim().strip_prefix("MCP HTTP listening on ").unwrap()
    );
    let http = reqwest::Client::builder()
        .no_proxy()
        .default_headers(
            [(
                reqwest::header::HOST,
                "slk.example.test:443".parse().unwrap(),
            )]
            .into_iter()
            .collect(),
        )
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    for host in ["127.0.0.1", "slk.example.test:444"] {
        let response = http
            .get(&url)
            .header("host", host)
            .bearer_auth(token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403);
    }
    let transport = StreamableHttpClientTransport::with_client(
        http,
        StreamableHttpClientTransportConfig::with_uri(url).auth_header(token),
    );
    let client = tokio::time::timeout(Duration::from_secs(5), ().serve(transport))
        .await
        .unwrap()
        .unwrap();
    let tools = client.list_tools(None).await.unwrap().tools;
    assert_eq!(tools.len(), 3);
    let write =
        serde_json::to_value(tools.iter().find(|tool| tool.name == "slk_write").unwrap()).unwrap();
    assert_eq!(write["_meta"]["anthropic/requiresUserInteraction"], true);
    let result = client
        .call_tool(
            CallToolRequestParams::new("slk_write").with_arguments(
                json!({"args": ["send", "C1", "--text", "not approved"]})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        result.structured_content.unwrap()["code"],
        "invalid_arguments"
    );
    assert_eq!(
        std::fs::read_to_string(config_file(home.path())).unwrap(),
        original
    );
    let mut changed: Value = serde_json::from_str(&original).unwrap();
    changed["profiles"]["work"]["identity"]["user_id"] = json!("U00000001");
    std::fs::write(config_file(home.path()), changed.to_string()).unwrap();
    for (tool, args) in [
        ("slk_read", vec!["auth", "status"]),
        (
            "slk_write",
            vec!["send", "C1", "--text", "not sent", "--yes"],
        ),
    ] {
        let result = client
            .call_tool(
                CallToolRequestParams::new(tool)
                    .with_arguments(json!({"args": args}).as_object().unwrap().clone()),
            )
            .await
            .unwrap();
        assert_eq!(
            result.structured_content.unwrap()["code"],
            "profile_conflict"
        );
    }
    client.cancel().await.unwrap();
    process.kill().await.unwrap();
    let mut errors = String::new();
    stderr.read_to_string(&mut errors).await.unwrap();
    assert!(errors.is_empty(), "{errors}");
    assert!(!ready.contains(token));
    let mut output = Vec::new();
    process
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut output)
        .await
        .unwrap();
    assert!(output.is_empty());
}
