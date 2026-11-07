use super::prepare;
use crate::auth::{Credentials, Kind};
use crate::slack::tests::{auth_response, client};
use serde_json::{Value, json};
use std::fs;
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn block_kit_previews_and_writes_preserve_complete_arrays_and_exact_targets() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    let original =
        json!([{"type": "rich_text", "elements": [{"type": "future", "text": "original 🦀"}]}]);
    let replacement = json!([
        {"type": "section", "text": {"type": "mrkdwn", "text": "*Details*"}, "future_field": {"keep": true}},
        {"type": "future_block", "payload": {"text": "literal $(never-run)\u{001b}]52;escaped", "count": 7}}
    ]);
    let temporary = tempfile::tempdir().unwrap();
    let file = temporary.path().join("blocks.json");
    for command in ["send", "reply", "edit"] {
        for preview in [true, false] {
            let server = MockServer::start().await;
            Mock::given(path("/api/auth.test"))
                .respond_with(ResponseTemplate::new(200).set_body_json(auth_response()))
                .mount(&server)
                .await;
            Mock::given(path("/api/conversations.history"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "ok": true, "messages": [{"ts": "1700000000.000001", "user": "U9", "text": "root"}]
                })))
                .mount(&server)
                .await;
            Mock::given(path("/api/conversations.replies"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "ok": true, "messages": [{
                        "ts": "1700000000.000002", "thread_ts": "1700000000.000001",
                        "user": "W87654321", "text": "Original fallback", "blocks": original
                    }]
                })))
                .mount(&server)
                .await;
            let channel = if command == "send" { "C9" } else { "C1" };
            let endpoint = if command == "edit" {
                "/api/chat.update"
            } else {
                "/api/chat.postMessage"
            };
            let thread = (command == "reply").then_some("1700000000.000001");
            Mock::given(path(endpoint))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "ok": true, "channel": channel, "ts": "1700000000.000002", "message": {"thread_ts": thread}
                })))
                .expect(u64::from(!preview))
                .mount(&server)
                .await;
            fs::write(&file, replacement.to_string()).unwrap();
            let prepared = prepare(&[
                command,
                if command == "send" { "C9" } else { "https://team.slack.com/archives/C1/p1700000000000002?thread_ts=1700000000.000001" },
                "--text", "Accessible fallback 🦀", "--blocks-file", file.to_str().unwrap(),
                "--profile", "work", "--json", if preview { "--dry-run" } else { "--yes" },
            ], Kind::User).unwrap();
            fs::write(&file, "[]").unwrap();
            let output = prepared
                .execute(&client(&server), &credentials)
                .await
                .unwrap();
            if preview {
                let mut text = Vec::new();
                output.write(false, &mut text).unwrap();
                let text = String::from_utf8(text).unwrap();
                assert!(text.contains("future_field"));
                assert!(text.contains("future_block"));
                assert!(!text.contains('\x1b'));
                let result = serde_json::to_value(output).unwrap();
                assert_eq!(result["blocks"], replacement);
                assert_eq!(result["text"], "Accessible fallback 🦀");
                if command == "edit" {
                    assert_eq!(result["message"]["blocks"], original);
                    assert_eq!(result["message"]["ts"], "1700000000.000002");
                } else {
                    assert_eq!(result["thread_ts"], json!(thread));
                }
            } else {
                let requests = server.received_requests().await.unwrap();
                let write = requests
                    .iter()
                    .find(|request| request.url.path() == endpoint)
                    .unwrap();
                let body = write.body_json::<Value>().unwrap();
                assert_eq!(body["blocks"], replacement);
                assert_eq!(body["text"], "Accessible fallback 🦀");
                assert_eq!(body["channel"], channel);
                assert_eq!(body["link_names"], false);
                assert_eq!(body["parse"], "none");
                if command == "edit" {
                    assert_eq!(body["ts"], "1700000000.000002");
                    assert!(body.get("thread_ts").is_none());
                } else {
                    assert_eq!(body["thread_ts"], json!(thread));
                    assert_eq!(body["reply_broadcast"], false);
                    assert_eq!(body["unfurl_links"], false);
                    assert_eq!(body["unfurl_media"], false);
                }
            }
        }
    }
}

#[tokio::test]
async fn explicit_empty_blocks_clear_content_but_do_not_bypass_ownership_or_message_safety() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let file = temporary.path().join("clear.json");
    fs::write(&file, "[]").unwrap();
    for (field, value, expected) in [
        ("text", Value::Null, None),
        ("user", json!("U9"), Some("message_not_owned")),
        ("bot_id", json!("B1"), Some("unsupported_message")),
        (
            "attachments",
            json!([{"text": "not represented"}]),
            Some("unsupported_message"),
        ),
        ("files", json!([{"id": "F1"}]), Some("unsupported_message")),
        (
            "subtype",
            json!("channel_join"),
            Some("unsupported_message"),
        ),
        (
            "blocks",
            json!({"unexpected": "shape"}),
            Some("unsupported_message"),
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(path("/api/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(auth_response()))
            .mount(&server)
            .await;
        let mut message = json!({
            "ts": "1700000000.000007", "user": "W87654321", "text": "fallback",
            "blocks": [{"type": "rich_text", "elements": [{"type": "future", "value": 3}]}]
        });
        message[field] = value;
        Mock::given(path("/api/conversations.history"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"ok": true, "messages": [message]})),
            )
            .mount(&server)
            .await;
        Mock::given(path("/api/chat.update"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"ok": true, "channel": "C1", "ts": "1700000000.000007"})),
            )
            .expect(u64::from(expected.is_none()))
            .mount(&server)
            .await;
        let result = prepare(
            &[
                "edit",
                "https://team.slack.com/archives/C1/p1700000000000007",
                "--text",
                "Intentional plain replacement",
                "--blocks-file",
                file.to_str().unwrap(),
                "--profile",
                "work",
                "--yes",
            ],
            Kind::User,
        )
        .unwrap()
        .execute(&client(&server), &credentials)
        .await;
        if let Some(code) = expected {
            assert_eq!(result.err().unwrap().report().code, code, "{field}");
        } else {
            assert_eq!(
                serde_json::to_value(result.unwrap()).unwrap()["blocks"],
                json!([])
            );
            let requests = server.received_requests().await.unwrap();
            let write = requests
                .iter()
                .find(|request| request.url.path() == "/api/chat.update")
                .unwrap();
            assert_eq!(write.body_json::<Value>().unwrap()["blocks"], json!([]));
        }
    }
}

#[test]
fn block_kit_input_requires_an_array_typed_objects_fallback_text_and_one_stdin_reader() {
    let temporary = tempfile::tempdir().unwrap();
    let file = temporary.path().join("blocks.json");
    for (input, valid) in [
        ("not JSON".to_owned(), false),
        ("null".to_owned(), false),
        ("{}".to_owned(), false),
        (json!([{"type": ""}]).to_string(), false),
        (json!([{"type": 9}]).to_string(), false),
        (json!([{}]).to_string(), false),
        (
            json!(vec![json!({"type": "divider"}); 51]).to_string(),
            false,
        ),
        (
            json!(vec![json!({"type": "divider"}); 50]).to_string(),
            true,
        ),
        ("[]".to_owned(), true),
    ] {
        fs::write(&file, input).unwrap();
        let result = prepare(
            &[
                "send",
                "C1",
                "--text",
                "fallback",
                "--blocks-file",
                file.to_str().unwrap(),
                "--dry-run",
            ],
            Kind::User,
        );
        assert_eq!(result.is_ok(), valid);
        if let Err(error) = result {
            assert_eq!(error.report().code, "invalid_arguments");
        }
    }
    for args in [
        vec![
            "send",
            "C1",
            "--text",
            " ",
            "--blocks-file",
            file.to_str().unwrap(),
            "--dry-run",
        ],
        vec![
            "send",
            "C1",
            "--file",
            "-",
            "--blocks-file",
            "-",
            "--dry-run",
        ],
    ] {
        assert_eq!(
            prepare(&args, Kind::User).err().unwrap().report().code,
            "invalid_arguments"
        );
    }
}

#[tokio::test]
async fn history_preserves_unknown_blocks_without_rendering_or_executing_them() {
    let server = MockServer::start().await;
    let blocks = json!([{"type": "future", "text": "literal $(never-run)\u{001b}]52;escaped", "extra": {"a": 9}}]);
    Mock::given(path("/api/auth.test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(auth_response()))
        .mount(&server)
        .await;
    Mock::given(path("/api/conversations.history"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true, "messages": [{"ts": "1700000000.000009", "blocks": blocks}]
        })))
        .mount(&server)
        .await;
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    let output = prepare(&["read", "C1"], Kind::User)
        .unwrap()
        .execute(&client(&server), &credentials)
        .await
        .unwrap();
    let mut text = Vec::new();
    output.write(false, &mut text).unwrap();
    let text = String::from_utf8(text).unwrap();
    assert!(!text.contains('\x1b'));
    assert!(text.contains("never-run"));
    assert_eq!(
        serde_json::to_value(output).unwrap()["data"][0]["blocks"],
        blocks
    );
}
