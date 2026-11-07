use super::prepare;
use crate::auth::{Credentials, Kind};
use crate::slack::tests::{auth_response, client};
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn identity(server: &MockServer) {
    Mock::given(path("/api/auth.test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(auth_response()))
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test]
async fn bot_modifications_require_the_bound_bot_and_supported_content() {
    let credentials = Credentials::parse(Kind::Bot, r#"{"token":"xoxb-test"}"#).unwrap();
    for (command, endpoint) in [("edit", "/api/chat.update"), ("delete", "/api/chat.delete")] {
        for (author, expected) in [
            (
                json!({"user": "W87654321", "bot_id": "B7", "subtype": "bot_message"}),
                None,
            ),
            (json!({"bot_id": "B7", "subtype": "bot_message"}), None),
            (
                json!({"user": "U9", "bot_id": "B8", "subtype": "bot_message"}),
                Some("message_not_owned"),
            ),
            (json!({"user": "W87654321"}), Some("unsupported_message")),
            (
                json!({"bot_id": "B7", "subtype": "future_event"}),
                Some("unsupported_message"),
            ),
        ] {
            let server = MockServer::start().await;
            let mut identity = auth_response();
            identity["bot_id"] = json!("B7");
            Mock::given(path("/api/auth.test"))
                .respond_with(ResponseTemplate::new(200).set_body_json(identity))
                .expect(1)
                .mount(&server)
                .await;
            let mut message = json!({"ts": "1700000000.000001", "text": "original"});
            message
                .as_object_mut()
                .unwrap()
                .extend(author.as_object().unwrap().clone());
            Mock::given(path("/api/conversations.history"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(json!({"ok": true, "messages": [message]})),
                )
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(path(endpoint))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(
                        json!({"ok": true, "channel": "C1", "ts": "1700000000.000001"}),
                    ),
                )
                .expect(u64::from(expected.is_none()))
                .mount(&server)
                .await;
            let mut args = vec![
                command,
                "https://team.slack.com/archives/C1/p1700000000000001",
                "--profile",
                "work",
                "--yes",
            ];
            if command == "edit" {
                args.extend(["--text", "new"]);
            }
            let result = prepare(&args, Kind::Bot)
                .unwrap()
                .execute(&client(&server), &credentials)
                .await;
            if let Some(code) = expected {
                assert_eq!(result.err().unwrap().report().code, code);
            } else {
                let result = serde_json::to_value(result.unwrap()).unwrap();
                assert_eq!(result["action"], command);
                assert_eq!(result["ts"], "1700000000.000001");
            }
        }
    }
}

#[tokio::test]
async fn edits_deletes_and_reactions_target_the_link_not_the_root() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for (command, options, endpoint, ts_field) in [
        (
            "edit",
            vec!["--text", "new $(literal) 🦀"],
            "/api/chat.update",
            "ts",
        ),
        ("delete", vec![], "/api/chat.delete", "ts"),
        (
            "react",
            vec!["thumbsup::skin-tone-3"],
            "/api/reactions.add",
            "timestamp",
        ),
        (
            "unreact",
            vec!["thumbsup::skin-tone-3"],
            "/api/reactions.remove",
            "timestamp",
        ),
        ("pin", vec![], "/api/pins.add", "timestamp"),
        ("unpin", vec![], "/api/pins.remove", "timestamp"),
    ] {
        for (url, thread, target_ts, read_method) in [
            (
                "https://team.slack.com/archives/C1/p1700000000000001",
                None,
                "1700000000.000001",
                "/api/conversations.history",
            ),
            (
                "https://team.slack.com/archives/C1/p1700000000000002?thread_ts=1700000000.000001",
                None,
                "1700000000.000002",
                "/api/conversations.replies",
            ),
            (
                "https://team.slack.com/archives/C1/p1700000000000002",
                Some("1700000000.000001"),
                "1700000000.000002",
                "/api/conversations.replies",
            ),
            (
                "https://team.slack.com/archives/C1/p1700000000000002",
                None,
                "1700000000.000002",
                "/api/conversations.history",
            ),
        ] {
            for dry_run in [true, false] {
                let server = MockServer::start().await;
                identity(&server).await;
                let mut message = json!({"ts": target_ts, "user": "W87654321", "text": "original message", "blocks": [], "attachments": []});
                if target_ts.ends_with('2') {
                    message["thread_ts"] = json!("1700000000.000001");
                    if read_method.ends_with("history") {
                        message["subtype"] = json!("thread_broadcast");
                    }
                }
                let messages = if read_method.ends_with("replies") {
                    vec![
                        json!({"ts": "1700000000.000001", "user": "U9999", "text": "root must not change"}),
                        message,
                    ]
                } else {
                    vec![message]
                };
                let mut read = Mock::given(method("GET"))
                    .and(path(read_method))
                    .and(query_param("channel", "C1"))
                    .and(query_param("oldest", target_ts))
                    .and(query_param("latest", target_ts))
                    .and(query_param("inclusive", "true"));
                if read_method.ends_with("replies") {
                    read = read.and(query_param("ts", "1700000000.000001"));
                }
                read.respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(json!({"ok": true, "messages": messages})),
                )
                .expect(1)
                .mount(&server)
                .await;
                Mock::given(method("POST"))
                    .and(path(endpoint))
                    .respond_with(
                        ResponseTemplate::new(200)
                            .set_body_json(json!({"ok": true, "channel": "C1", "ts": target_ts})),
                    )
                    .expect(if dry_run { 0 } else { 1 })
                    .mount(&server)
                    .await;
                let mut args = vec![command, url];
                args.extend(options.iter().copied());
                if let Some(root) = thread {
                    args.extend(["--thread", root]);
                }
                args.extend(["--profile", "work", "--json"]);
                args.push(if dry_run { "--dry-run" } else { "--yes" });
                let output = prepare(&args, Kind::User)
                    .unwrap()
                    .execute(&client(&server), &credentials)
                    .await
                    .unwrap();
                let result = serde_json::to_value(output).unwrap();
                assert_eq!(result["action"], command);
                if dry_run {
                    assert_eq!(result["dry_run"], true);
                    assert_eq!(result["message"]["ts"], target_ts);
                    assert_eq!(result["message"]["text"], "original message");
                    assert_eq!(result["user_id"], "W87654321");
                } else {
                    assert_eq!(result["channel_id"], "C1");
                    assert_eq!(result["ts"], target_ts);
                }
                if command == "edit" {
                    assert_eq!(result["text"], "new $(literal) 🦀");
                }
                let requests = server.received_requests().await.unwrap();
                assert_eq!(requests.len(), if dry_run { 2 } else { 3 });
                if !dry_run {
                    let request = requests
                        .iter()
                        .find(|request| request.url.path() == endpoint)
                        .unwrap();
                    let mut expected = json!({"channel": "C1"});
                    expected[ts_field] = json!(target_ts);
                    match command {
                        "edit" => {
                            expected["text"] = json!("new $(literal) 🦀");
                            expected["parse"] = json!("none");
                            expected["link_names"] = json!(false);
                        }
                        "react" | "unreact" => expected["name"] = json!("thumbsup::skin-tone-3"),
                        _ => (),
                    }
                    assert_eq!(request.body_json::<Value>().unwrap(), expected);
                }
            }
        }
    }
}

#[tokio::test]
async fn own_text_mutations_reject_other_authors_and_nontext_content_before_writing() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for command in ["edit", "delete"] {
        for (field, value, expected) in [
            ("user", json!("U9"), "message_not_owned"),
            ("user", Value::Null, "message_not_owned"),
            ("bot_id", json!("B1"), "unsupported_message"),
            (
                "blocks",
                json!([{"type": "future_block"}]),
                "unsupported_message",
            ),
            (
                "attachments",
                json!([{"text": "not previewed"}]),
                "unsupported_message",
            ),
            ("files", json!([{"id": "F1"}]), "unsupported_message"),
            ("subtype", json!("channel_join"), "unsupported_message"),
            ("subtype", json!("future_event"), "unsupported_message"),
        ] {
            let server = MockServer::start().await;
            identity(&server).await;
            let mut message =
                json!({"ts": "1700000000.000001", "user": "W87654321", "text": "original"});
            message[field] = value;
            Mock::given(path("/api/conversations.history"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(json!({"ok": true, "messages": [message]})),
                )
                .expect(1)
                .mount(&server)
                .await;
            let mut args = vec![
                command,
                "https://team.slack.com/archives/C1/p1700000000000001",
                "--profile",
                "work",
                "--yes",
            ];
            if command == "edit" {
                args.extend(["--text", "new"]);
            }
            let error = prepare(&args, Kind::User)
                .unwrap()
                .execute(&client(&server), &credentials)
                .await
                .err()
                .unwrap();
            assert_eq!(error.report().code, expected, "{command}, {field}");
            assert_eq!(server.received_requests().await.unwrap().len(), 2);
        }
    }
}

#[tokio::test]
async fn inaccessible_deleted_and_unrelated_messages_never_become_mutation_targets() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for (body, expected) in [
        (json!({"ok": true, "messages": []}), "message_unresolved"),
        (
            json!({"ok": true, "messages": [{"ts": "1700000000.000001", "user": "W87654321", "text": "root"}]}),
            "message_unresolved",
        ),
        (
            json!({"ok": true, "messages": [{"ts": "1700000000.000002", "thread_ts": "1699999999.999999", "user": "W87654321", "text": "unrelated"}]}),
            "message_unresolved",
        ),
        (
            json!({"ok": false, "error": "channel_not_found"}),
            "slack_error",
        ),
        (
            json!({"ok": false, "error": "missing_scope"}),
            "slack_error",
        ),
    ] {
        let server = MockServer::start().await;
        identity(&server).await;
        Mock::given(path("/api/conversations.replies"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;
        let error = prepare(
            &[
                "delete",
                "https://team.slack.com/archives/C1/p1700000000000002?thread_ts=1700000000.000001",
                "--profile",
                "work",
                "--yes",
            ],
            Kind::User,
        )
        .unwrap()
        .execute(&client(&server), &credentials)
        .await
        .err()
        .unwrap();
        assert_eq!(error.report().code, expected);
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }
}

#[tokio::test]
async fn reactions_accept_other_authors_and_keep_slack_permission_semantics() {
    let credentials = Credentials::parse(
        Kind::Browser,
        r#"{"token":"xoxc-test","cookie":"xoxd-test","workspace_url":"https://team.slack.com"}"#,
    )
    .unwrap();
    for (command, endpoint, responses) in [
        (
            "react",
            "/api/reactions.add",
            [None, Some("already_reacted"), Some("not_reactable")],
        ),
        (
            "unreact",
            "/api/reactions.remove",
            [None, Some("no_reaction"), Some("missing_scope")],
        ),
    ] {
        for error in responses {
            let server = MockServer::start().await;
            identity(&server).await;
            Mock::given(path("/api/conversations.history"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true, "messages": [{"ts": "1700000000.000001", "bot_id": "B9", "subtype": "future_type", "blocks": [{"type": "future_block"}]}]})))
                .expect(1).mount(&server).await;
            let body = error.map_or_else(
                || json!({"ok": true}),
                |code| json!({"ok": false, "error": code}),
            );
            Mock::given(path(endpoint))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .expect(1)
                .mount(&server)
                .await;
            let result = prepare(
                &[
                    command,
                    "https://team.slack.com/archives/C1/p1700000000000001",
                    "wave",
                    "--profile",
                    "work",
                    "--yes",
                ],
                Kind::Browser,
            )
            .unwrap()
            .execute(&client(&server), &credentials)
            .await;
            if let Some(expected) = error {
                let error = result.err().unwrap();
                assert_eq!(error.report().code, "slack_error");
                assert_eq!(error.report().slack_error, Some(expected));
            } else {
                let output = serde_json::to_value(result.unwrap()).unwrap();
                assert_eq!(
                    output,
                    json!({"channel_id": "C1", "ts": "1700000000.000001", "action": command, "name": "wave"})
                );
            }
            assert_eq!(server.received_requests().await.unwrap().len(), 3);
        }
    }
}

#[tokio::test]
async fn invalid_mutation_acknowledgements_are_unknown_not_success_or_rejection() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for (command, endpoint) in [("edit", "/api/chat.update"), ("delete", "/api/chat.delete")] {
        for response in [
            ResponseTemplate::new(503),
            ResponseTemplate::new(200).set_body_string("xoxp-test invalid JSON"),
            ResponseTemplate::new(200).set_body_json(json!({"ok": true})),
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok": true, "channel": "C1", "ts": "1700000000.000002"})),
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok": true, "channel": "C9", "ts": "1700000000.000001"})),
        ] {
            let server = MockServer::start().await;
            identity(&server).await;
            Mock::given(path("/api/conversations.history"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true, "messages": [{"ts": "1700000000.000001", "user": "W87654321", "text": "original"}]})))
                .expect(1).mount(&server).await;
            Mock::given(path(endpoint))
                .respond_with(response)
                .expect(1)
                .mount(&server)
                .await;
            let mut args = vec![
                command,
                "https://team.slack.com/archives/C1/p1700000000000001",
                "--profile",
                "work",
                "--yes",
            ];
            if command == "edit" {
                args.extend(["--text", "new"]);
            }
            let error = prepare(&args, Kind::User)
                .unwrap()
                .execute(&client(&server), &credentials)
                .await
                .err()
                .unwrap();
            assert_eq!(error.report().code, "write_outcome_unknown");
            assert!(
                !serde_json::to_string(&error.report())
                    .unwrap()
                    .contains("xoxp-test")
            );
            assert_eq!(server.received_requests().await.unwrap().len(), 3);
        }
    }
}

#[tokio::test]
async fn pins_preserve_opaque_items_and_slack_rejections_without_faking_later() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    let server = MockServer::start().await;
    identity(&server).await;
    let items = json!([
        {"type": "message", "channel": "C1", "message": {"ts": "1700000000.000002", "text": "pinned 🦀\u{001b}]52;hidden"}},
        {"type": "file", "file": {"id": "F1"}},
        {"type": "future", "payload": {"nested": true}}
    ]);
    Mock::given(method("GET"))
        .and(path("/api/pins.list"))
        .and(query_param("channel", "C1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true, "items": items})))
        .expect(1)
        .mount(&server)
        .await;
    let result = prepare(&["pins", "C1", "--profile", "work"], Kind::User)
        .unwrap()
        .execute(&client(&server), &credentials)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        json!({"channel_id": "C1", "data": items})
    );
    let mut text = Vec::new();
    result.write(false, &mut text).unwrap();
    assert!(!text.contains(&0x1b));
    assert_eq!(server.received_requests().await.unwrap().len(), 2);

    for (command, endpoint, code) in [
        ("pin", "/api/pins.add", "already_pinned"),
        ("unpin", "/api/pins.remove", "not_pinned"),
    ] {
        let server = MockServer::start().await;
        identity(&server).await;
        Mock::given(path("/api/conversations.history"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true, "messages": [{"ts": "1700000000.000002", "user": "U9", "text": "someone else's message"}]})))
            .expect(1).mount(&server).await;
        Mock::given(path(endpoint))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"ok": false, "error": code})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let error = prepare(
            &[
                command,
                "https://team.slack.com/archives/C1/p1700000000000002",
                "--profile",
                "work",
                "--yes",
            ],
            Kind::User,
        )
        .unwrap()
        .execute(&client(&server), &credentials)
        .await
        .err()
        .unwrap();
        assert_eq!(error.report().slack_error, Some(code));
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
    }
}
