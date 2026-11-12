use super::{Cli, Server, http::Http, slack_server};
use crate::error::Error;
use crate::testing::auth_response;
use base64::{Engine, prelude::BASE64_STANDARD};
use rmcp::{
    ServerHandler, ServiceExt,
    model::{CallToolRequestParams, CallToolResponse, NumberOrString},
    service::RequestContext,
};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn mcp_writes_declare_mandatory_confirmation_and_preserve_payloads_previews_and_failures() {
    let api = MockServer::start().await;
    Mock::given(path("/api/auth.test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(auth_response()))
        .mount(&api)
        .await;
    let text = "Literal $(never-run) 🦀\nsecond line";
    let blocks = json!([{"type": "section", "text": {"type": "mrkdwn", "text": "Approved body"}}]);
    Mock::given(method("POST")).and(path("/api/chat.postMessage"))
        .and(body_partial_json(json!({"channel": "C9", "text": text, "blocks": blocks, "parse": "none", "link_names": false})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true, "channel": "C9", "ts": "1700000000.000007", "message": {}})))
        .expect(1).mount(&api).await;
    Mock::given(path("/api/chat.postMessage"))
        .and(body_partial_json(json!({"channel": "C8"})))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "17")
                .set_body_json(json!({"ok": false, "error": "ratelimited"})),
        )
        .expect(1)
        .mount(&api)
        .await;
    Mock::given(path("/api/chat.postMessage"))
        .and(body_partial_json(json!({"channel": "C7"})))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&api)
        .await;
    let http = Http::new(slack_server(&api)).await;
    let tools = http.client.list_tools(None).await.unwrap().tools;
    assert_eq!(tools.len(), 3);
    let tool =
        serde_json::to_value(tools.iter().find(|tool| tool.name == "slk_write").unwrap()).unwrap();
    assert_eq!(tool["_meta"]["anthropic/requiresUserInteraction"], true);
    assert_eq!(tool["annotations"]["readOnlyHint"], false);
    assert_eq!(tool["annotations"]["idempotentHint"], false);
    assert!(api.received_requests().await.unwrap().is_empty());

    for flags in [vec![], vec!["--profile", "other", "--yes"]] {
        let mut args = vec!["send", "C9", "--text", text];
        args.extend(flags);
        let result = http.call("slk_write", json!({"args": args})).await;
        assert_eq!(
            result.structured_content.unwrap()["code"],
            "invalid_arguments"
        );
    }
    assert!(api.received_requests().await.unwrap().is_empty());
    for (flag, preview) in [("--dry-run", true), ("--yes", false)] {
        let result = http.call("slk_write", json!({
            "args": ["send", "C9", "--file", "body.txt", "--blocks-file", "blocks.json", flag],
            "files": {"body.txt": BASE64_STANDARD.encode(text), "blocks.json": BASE64_STANDARD.encode(blocks.to_string())}
        })).await;
        assert_eq!(result.is_error, Some(false));
        let value = result.structured_content.unwrap();
        assert_eq!(value["channel_id"], "C9");
        if preview {
            assert_eq!(value["dry_run"], true);
            assert_eq!(value["profile"], "work");
            assert_eq!(value["text"], text);
            assert_eq!(value["blocks"], blocks);
            assert_eq!(api.received_requests().await.unwrap().len(), 1);
        } else {
            assert_eq!(value["ts"], "1700000000.000007");
        }
        assert!(!value.to_string().contains("xoxp-test"));
    }
    for (channel, code) in [("C8", "rate_limited"), ("C7", "write_outcome_unknown")] {
        let result = http
            .call(
                "slk_write",
                json!({"args": ["send", channel, "--text", "body", "--yes"]}),
            )
            .await;
        assert_eq!(result.is_error, Some(true));
        let value = result.structured_content.unwrap();
        assert_eq!(value["code"], code);
        if channel == "C8" {
            assert_eq!(value["retry_after"], 17);
        }
    }
    assert_eq!(api.received_requests().await.unwrap().len(), 7);
}

#[tokio::test]
async fn mcp_write_admission_covers_business_commands_without_management_or_identity_overrides() {
    let http = Http::new(Server::new(
        "work".into(),
        |_| async { Err(Error::CredentialMissing) },
        true,
    ))
    .await;
    let link = "https://team.slack.com/archives/C1/p1700000000000007";
    for args in [
        vec!["send", "C1", "--text", "body", "--yes"],
        vec!["reply", link, "--text", "body", "--yes"],
        vec!["edit", link, "--text", "replacement", "--yes"],
        vec!["delete", link, "--yes"],
        vec!["react", link, "wave", "--yes"],
        vec!["unreact", link, "wave", "--yes"],
        vec!["pin", link, "--yes"],
        vec!["unpin", link, "--yes"],
        vec!["dm", "U1", "--yes"],
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
        vec!["channel", "mark", "C1", "1700000000.000007", "--yes"],
        vec!["group-dm", "open", "U1", "U2", "--yes"],
        vec!["group-dm", "resume", "G1", "--yes"],
        vec!["group-dm", "close", "G1", "--yes"],
        vec![
            "schedule",
            "create",
            "C1",
            "--at",
            "2030-01-01T00:00:00Z",
            "--text",
            "later",
            "--yes",
        ],
        vec![
            "schedule",
            "reply",
            link,
            "--at",
            "2030-01-01T00:00:00Z",
            "--text",
            "later",
            "--yes",
        ],
        vec!["schedule", "cancel", "C1", "Q7", "--yes"],
        vec!["canvas", "append", "F1", "--text", "body", "--yes"],
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
        vec![
            "messages",
            "send",
            "--recipient-id",
            "C1",
            "--message",
            "body",
            "--yes",
        ],
        vec![
            "messages",
            "delete",
            "--channel-id",
            "C1",
            "--timestamp",
            "1700000000.000007",
            "--yes",
        ],
        vec!["files", "download", "F1", "--output", "result.bin"],
    ] {
        let result = http.call("slk_write", json!({"args": args})).await;
        assert_eq!(
            result.structured_content.unwrap()["code"],
            "credential_missing",
            "{args:?}"
        );
        let result = http.call("slk_read", json!({"args": args})).await;
        assert_eq!(
            result.structured_content.unwrap()["code"],
            "invalid_arguments",
            "{args:?}"
        );
    }
    for args in [
        vec!["auth", "logout"],
        vec!["auth", "refresh"],
        vec!["auth", "status"],
        vec!["update", "--yes"],
        vec!["mcp"],
        vec!["read", "C1"],
        vec!["send", "C1", "--text", "body", "--yes", "--profile=work"],
        vec![
            "send",
            "C1",
            "--text",
            "body",
            "--yes",
            "--session-env",
            "SECRET",
            "--profile",
            "work",
        ],
        vec![
            "send",
            "C1",
            "--text",
            "body",
            "--yes",
            "--config-dir",
            "/private",
        ],
        vec![
            "messages",
            "--workspace",
            "work",
            "send",
            "--recipient-id",
            "C1",
            "--message",
            "body",
            "--yes",
        ],
        vec![
            "files",
            "download",
            "F1",
            "--output",
            "result.bin",
            "--workspace=work",
        ],
    ] {
        let result = http.call("slk_write", json!({"args": args})).await;
        assert_eq!(
            result.structured_content.unwrap()["code"],
            "invalid_arguments",
            "{args:?}"
        );
    }
}

#[tokio::test]
async fn mcp_cancellation_never_claims_an_in_flight_write_was_not_sent() {
    let started = Arc::new(tokio::sync::Notify::new());
    let notify = Arc::clone(&started);
    let handler = Server::new(
        "work".into(),
        move |_: Cli| {
            let notify = Arc::clone(&notify);
            async move {
                notify.notify_one();
                std::future::pending().await
            }
        },
        true,
    );
    let (server_io, client_io) = tokio::io::duplex(8192);
    let (server, client) = tokio::join!(handler.clone().serve(server_io), ().serve(client_io));
    let server = server.unwrap();
    let client = client.unwrap();
    for (flag, code) in [
        ("--yes", "write_outcome_unknown"),
        ("--dry-run", "cancelled"),
    ] {
        let context = RequestContext::new(NumberOrString::Number(7), server.peer().clone());
        let cancellation = context.ct.clone();
        let request = handler.call_tool(
            CallToolRequestParams::new("slk_write").with_arguments(
                json!({"args": ["send", "C1", "--text", "body", flag]})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
            context,
        );
        let cancel = async {
            started.notified().await;
            cancellation.cancel();
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(request, cancel)
        })
        .await
        .unwrap();
        let CallToolResponse::Complete(result) = result.unwrap() else {
            panic!("expected a completed tool response");
        };
        assert_eq!(result.structured_content.unwrap()["code"], code);
        assert_eq!(result.is_error, Some(true));
    }
    client.cancel().await.unwrap();
    server.waiting().await.unwrap();
}
