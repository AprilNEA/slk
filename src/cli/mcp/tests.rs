use super::{Cli, Server};
use crate::auth::{Credentials, Kind, Store};
use crate::cli::Command;
use crate::error::Error;
use crate::testing::{self, auth_response, client};
use rmcp::{ServiceExt, model::CallToolRequestParams};
use serde_json::{Value, json};
use std::sync::Arc;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod files;
mod http;
mod writes;

fn slack_server(api: &MockServer) -> impl rmcp::ServerHandler + Clone {
    let store = Store::memory(
        "work",
        &json!({
            "kind": "user",
            "identity": {"workspace_url": "https://team.slack.com/", "team_id": "T12345678", "user_id": "W87654321"},
            "credentials": {"token": "xoxp-test"},
            "settings": {"files": {"max_bytes": 8}}
        })
        .to_string(),
    )
    .unwrap();
    let credentials = store.credentials("work").unwrap();
    let context = Arc::new((store, client(api), credentials));
    Server::new(
        "work".into(),
        move |cli: Cli| {
            let context = Arc::clone(&context);
            async move {
                let (store, slack, credentials) = context.as_ref();
                let profile = cli.profile.as_deref();
                match cli.command {
                    Command::Messages(command) => {
                        command
                            .prepare(store, profile, cli.json)?
                            .execute(slack, credentials)
                            .await
                    }
                    Command::Compatibility(command) => {
                        command
                            .prepare(store, profile, cli.json)?
                            .execute(slack, credentials)
                            .await
                    }
                    Command::Users(command) => {
                        command
                            .prepare(store, profile, cli.json)?
                            .execute(slack, credentials)
                            .await
                    }
                    Command::Conversations(command) => {
                        command
                            .prepare(store, profile, cli.json)?
                            .execute(slack, credentials)
                            .await
                    }
                    Command::Files { command, .. } => {
                        command
                            .prepare(store, profile, cli.json)?
                            .execute(slack, credentials)
                            .await
                    }
                    Command::Schedule { command } => {
                        command
                            .prepare(store, profile, cli.json)?
                            .execute(slack, credentials)
                            .await
                    }
                    Command::Canvas { command } => {
                        command
                            .prepare(store, profile, cli.json)?
                            .execute(slack, credentials)
                            .await
                    }
                    _ => panic!("unexpected management command"),
                }
            }
        },
        true,
    )
}

#[tokio::test]
async fn mcp_reads_preserve_json_pagination_and_errors_without_retries() {
    let api = MockServer::start().await;
    Mock::given(path("/api/auth.test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(auth_response()))
        .expect(2)
        .mount(&api)
        .await;
    let blocks = json!([{"type": "future_block", "extra": {"value": 23}}]);
    Mock::given(method("GET"))
        .and(path("/api/conversations.history"))
        .and(query_param("channel", "C1"))
        .and(query_param("limit", "2"))
        .and(query_param("cursor", "FIRST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "messages": [{"ts": "1700000000.000007", "text": "literal $(never-run)\u{1b}[31m", "blocks": blocks}],
            "has_more": true,
            "response_metadata": {"next_cursor": "SECOND"}
        })))
        .expect(1)
        .mount(&api)
        .await;
    Mock::given(path("/api/conversations.history"))
        .and(query_param("cursor", "SECOND"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "17")
                .set_body_json(json!({"ok": false, "error": "ratelimited"})),
        )
        .expect(1)
        .mount(&api)
        .await;

    let (_directory, store) = testing::store(&["work", "another"], Kind::User, json!({}));
    let context = Arc::new((
        store,
        client(&api),
        Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap(),
    ));
    let server = Server::new(
        "work".into(),
        move |cli: Cli| {
            let context = Arc::clone(&context);
            async move {
                let (store, slack, credentials) = context.as_ref();
                let Command::Messages(command) = cli.command else {
                    panic!("expected a message read");
                };
                command
                    .prepare(store, cli.profile.as_deref(), cli.json)?
                    .execute(slack, credentials)
                    .await
            }
        },
        false,
    );
    let (server_io, client_io) = tokio::io::duplex(8192);
    let server_task = tokio::spawn(async move {
        server
            .serve(server_io)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap()
    });
    let client = ().serve(client_io).await.unwrap();
    let tools = client.list_tools(None).await.unwrap().tools;
    assert_eq!(tools.len(), 2);
    assert!(
        tools
            .iter()
            .all(|tool| tool.annotations.as_ref().unwrap().read_only_hint == Some(true))
    );
    let read = tools.iter().find(|tool| tool.name == "slk_read").unwrap();
    assert_eq!(read.input_schema["properties"]["args"]["type"], "array");
    assert!(api.received_requests().await.unwrap().is_empty());

    let result = client
        .call_tool(
            CallToolRequestParams::new("slk_read").with_arguments(
                json!({"args": ["read", "C1", "--limit", "2", "--cursor", "FIRST"]})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(false));
    let value = result.structured_content.unwrap();
    assert_eq!(value["data"][0]["ts"], "1700000000.000007");
    assert_eq!(value["data"][0]["text"], "literal $(never-run)\u{1b}[31m");
    assert_eq!(value["data"][0]["blocks"], blocks);
    assert_eq!(
        value["pagination"],
        json!({"next_cursor": "SECOND", "has_more": true})
    );
    assert_eq!(
        serde_json::from_str::<Value>(&result.content[0].as_text().unwrap().text).unwrap(),
        value
    );
    assert_eq!(api.received_requests().await.unwrap().len(), 2);

    let result = client
        .call_tool(
            CallToolRequestParams::new("slk_read").with_arguments(
                json!({"args": ["read", "C1", "--cursor", "SECOND"]})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    let value = result.structured_content.unwrap();
    assert_eq!(value["code"], "rate_limited");
    assert_eq!(value["retry_after"], 17);
    assert!(!value.to_string().contains("xoxp"));
    assert_eq!(api.received_requests().await.unwrap().len(), 4);
    client.cancel().await.unwrap();
    server_task.await.unwrap();
}

#[tokio::test]
async fn mcp_rejects_writes_local_io_and_profile_overrides_before_execution() {
    let server = Server::new(
        "work".into(),
        |_| async { Err(Error::CredentialMissing) },
        false,
    );
    let (server_io, client_io) = tokio::io::duplex(8192);
    let server_task = tokio::spawn(async move {
        server
            .serve(server_io)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap()
    });
    let client = ().serve(client_io).await.unwrap();
    let link = "https://team.slack.com/archives/C1/p1700000000000001";
    for args in [
        vec!["mcp"],
        vec!["update", "--check"],
        vec!["update", "--yes"],
        vec!["conversations", "list", "--workspace", "work"],
        vec!["auth", "whoami", "--workspace", "work"],
        vec!["search", "messages", "deploy", "--workspace", "work"],
        vec!["users", "info", "U1", "--workspace", "work"],
        vec!["files", "info", "F1", "--workspace", "work"],
        vec!["read", "C1", "--config-dir", "/unreadable/config"],
        vec![
            "read",
            "C1",
            "--session-env",
            "SLK_TEST_SESSION",
            "--profile",
            "work",
        ],
        vec!["read", "C1", "--session-stdin", "--profile", "work"],
        vec![
            "messages",
            "send",
            "--recipient-id",
            "C1",
            "--message-file",
            "-",
            "--dry-run",
        ],
        vec!["auth", "list"],
        vec!["auth", "logout"],
        vec!["auth", "refresh"],
        vec!["auth", "login", "--kind", "user", "--stdin"],
        vec!["auth", "browser", "--workspace", "https://team.slack.com"],
        vec![
            "auth",
            "oauth",
            "--client-id",
            "123",
            "--redirect-uri",
            "http://localhost:1234/callback",
            "--workspace",
            "https://team.slack.com",
            "--scope",
            "chat:write",
        ],
        vec!["send", "C1", "--text", "body", "--yes"],
        vec!["send", "C1", "--file", "-", "--dry-run"],
        vec!["reply", link, "--text", "body", "--dry-run"],
        vec!["edit", link, "--file", "/unreadable/file", "--yes"],
        vec!["delete", link, "--yes"],
        vec!["react", link, "wave", "--yes"],
        vec!["unreact", link, "wave", "--yes"],
        vec!["pin", link, "--yes"],
        vec!["unpin", link, "--yes"],
        vec!["dm", "U1", "--dry-run"],
        vec!["files", "upload", "/unreadable/file", "--dry-run"],
        vec!["files", "download", "F1", "--output", "/unwritable/file"],
        vec![
            "schedule",
            "create",
            "C1",
            "--at",
            "2026-11-07T01:00:00Z",
            "--text",
            "body",
            "--dry-run",
        ],
        vec![
            "schedule",
            "reply",
            link,
            "--at",
            "2026-11-07T01:00:00Z",
            "--text",
            "body",
            "--dry-run",
        ],
        vec!["schedule", "cancel", "C1", "Q1", "--yes"],
        vec![
            "channel",
            "create",
            "example",
            "--visibility",
            "private",
            "--yes",
        ],
        vec!["channel", "join", "C1", "--yes"],
        vec!["channel", "leave", "C1", "--yes"],
        vec!["channel", "archive", "C1", "--yes"],
        vec!["channel", "topic", "C1", "topic", "--yes"],
        vec!["channel", "mark", "C1", "1700000000.000001", "--yes"],
        vec!["group-dm", "open", "U1", "U2", "--yes"],
        vec!["group-dm", "resume", "G1", "--yes"],
        vec!["group-dm", "close", "G1", "--yes"],
        vec!["canvas", "append", "F1", "--file", "-", "--dry-run"],
        vec!["canvas", "prepend", "F1", "--text", "body", "--yes"],
        vec![
            "canvas",
            "insert-before",
            "F1",
            "temp:C:one",
            "--text",
            "body",
            "--yes",
        ],
        vec![
            "canvas",
            "insert-after",
            "F1",
            "temp:C:one",
            "--text",
            "body",
            "--yes",
        ],
        vec![
            "canvas",
            "replace-section",
            "F1",
            "temp:C:one",
            "--text",
            "body",
            "--yes",
        ],
        vec!["canvas", "delete-section", "F1", "temp:C:one", "--yes"],
        vec!["read", "C1", "--profile", "other"],
        vec!["--profile=work", "read", "C1"],
        vec!["read", "C1", "--token", "xoxp-secret"],
        vec!["read", "C1", "--limit", "0"],
    ] {
        let result = client
            .call_tool(
                CallToolRequestParams::new("slk_read")
                    .with_arguments(json!({"args": args}).as_object().unwrap().clone()),
            )
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true), "{args:?}");
        let value = result.structured_content.unwrap();
        assert_eq!(value["code"], "invalid_arguments", "{args:?}");
        assert!(!value.to_string().contains("xoxp-secret"));
    }
    for args in [
        vec!["auth", "status"],
        vec!["auth", "whoami"],
        vec!["conversations", "list"],
        vec!["conversations", "read", "C1", "--all", "--max-pages", "2"],
        vec!["conversations", "get", "--permalink", link],
        vec!["conversations", "members", "list", "C1"],
        vec!["get", link],
        vec!["search", "messages", "deploy"],
        vec!["channels"],
        vec!["read", "C1"],
        vec!["thread", link],
        vec!["search", "literal $(never-run)"],
        vec!["pins", "C1"],
        vec!["users", "list"],
        vec!["users", "info", "U1"],
        vec!["users", "lookup", "user@example.test"],
        vec!["resolve", "user", "name"],
        vec!["resolve", "channel", "name"],
        vec!["files", "info", "F1"],
        vec!["schedule", "list", "C1"],
        vec!["channel", "info", "C1"],
        vec!["channel", "members", "G1"],
        vec!["group-dm", "list"],
        vec!["unread"],
        vec!["workspace"],
        vec!["canvas", "read", "F1"],
        vec!["canvas", "sections", "F1", "--contains", "example"],
    ] {
        let result = client
            .call_tool(
                CallToolRequestParams::new("slk_read")
                    .with_arguments(json!({"args": args}).as_object().unwrap().clone()),
            )
            .await
            .unwrap();
        assert_eq!(
            result.structured_content.unwrap()["code"],
            "credential_missing",
            "{args:?}"
        );
    }
    client.cancel().await.unwrap();
    server_task.await.unwrap();
}
