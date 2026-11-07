use super::Prepared;
use crate::auth::{Credentials, Kind, Store};
use crate::cli::{Cli, Command};
use crate::error::{Error, Result};
use crate::slack::tests::{auth_response, client};
use clap::Parser;
use serde_json::{Value, json};
use std::fs;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn prepare(args: &[&str]) -> Result<Prepared> {
    let home = tempfile::tempdir().unwrap();
    fs::write(home.path().join("config.json"), json!({
        "settings": {"page_size": 7},
        "profiles": {"work": {"kind": "user", "identity": {
            "workspace_url": "https://team.slack.com", "team_id": "T12345678", "user_id": "W87654321"
        }, "keyring_ref": "00112233-4455-4677-8899-aabbccddeeff"}}
    }).to_string()).unwrap();
    let store = Store::load(home.path().to_owned()).unwrap();
    let cli = Cli::try_parse_from(std::iter::once("slk").chain(args.iter().copied()))
        .map_err(|_| Error::Arguments("Invalid test command."))?;
    let Command::Conversations(command) = cli.command else {
        panic!("expected conversation command")
    };
    command.prepare(&store, cli.profile.as_deref(), cli.json)
}

async fn identity(server: &MockServer) {
    Mock::given(path("/api/auth.test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(auth_response()))
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test]
async fn read_state_keeps_unknown_separate_from_zero_and_never_opens_or_marks_conversations() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for group in [false, true] {
        let server = MockServer::start().await;
        identity(&server).await;
        let channels = if group {
            json!([{"id": "G3", "is_mpim": true, "is_private": true, "name": "group", "unread_count_display": 4}])
        } else {
            json!([
                {"id": "C1", "is_channel": true, "name": "visible\u{001b}]52;hidden", "topic": {"value": "safe topic"}},
                {"id": "D2", "is_im": true, "user": "U8", "last_read": "1700000000.000003", "unread_count": 0, "unread_count_display": 0}
            ])
        };
        Mock::given(method("GET")).and(path("/api/users.conversations"))
            .and(query_param("types", if group { "mpim" } else { "public_channel,private_channel,mpim,im" }))
            .and(query_param("limit", "7")).and(query_param("cursor", "next+/="))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true, "channels": channels, "response_metadata": {"next_cursor": "more"}})))
            .expect(1).mount(&server).await;
        let mut args = if group {
            vec!["group-dm", "list"]
        } else {
            vec!["unread"]
        };
        args.extend(["--profile", "work", "--cursor", "next+/="]);
        let result = prepare(&args)
            .unwrap()
            .execute(&client(&server), &credentials)
            .await
            .unwrap();
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["pagination"]["next_cursor"], "more");
        assert!(value["data"][0].get("unread_count").is_none());
        if group {
            assert_eq!(value["data"][0]["conversation_type"], "mpim");
            assert_eq!(value["data"][0]["unread_count_display"], 4);
        } else {
            assert_eq!(value["data"][1]["unread_count"], 0);
            assert_eq!(value["data"][1]["last_read"], "1700000000.000003");
        }
        let mut output = Vec::new();
        result.write(false, &mut output).unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("Unread count: unknown"));
        assert!(output.contains("Next cursor: more"));
        assert!(!output.contains('\x1b'));
        assert!(!output.contains("hidden"));
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }
}

#[tokio::test]
async fn conversation_mutations_preview_and_apply_the_explicit_action_once() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for (args, endpoint, body) in [
        (
            vec!["channel", "join", "C1"],
            "conversations.join",
            json!({"channel": "C1"}),
        ),
        (
            vec!["channel", "leave", "C1"],
            "conversations.leave",
            json!({"channel": "C1"}),
        ),
        (
            vec!["channel", "archive", "C1"],
            "conversations.archive",
            json!({"channel": "C1"}),
        ),
        (
            vec!["channel", "topic", "C1", "new 🦀"],
            "conversations.setTopic",
            json!({"channel": "C1", "topic": "new 🦀"}),
        ),
        (
            vec!["channel", "mark", "C1", "1700000000.000002"],
            "conversations.mark",
            json!({"channel": "C1", "ts": "1700000000.000002"}),
        ),
        (
            vec!["group-dm", "resume", "G1"],
            "conversations.open",
            json!({"channel": "G1", "return_im": true}),
        ),
        (
            vec!["group-dm", "close", "G1"],
            "conversations.close",
            json!({"channel": "G1"}),
        ),
    ] {
        for preview in [true, false] {
            let server = MockServer::start().await;
            identity(&server).await;
            let group = args[0] == "group-dm";
            let channel = json!({"id": if group { "G1" } else { "C1" }, "name": "target", "is_channel": !group, "is_mpim": group, "topic": {"value": "old\u{001b}]52;hidden"}, "last_read": "1700000000.000001"});
            Mock::given(method("GET"))
                .and(path("/api/conversations.info"))
                .and(query_param("channel", args[2]))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(json!({"ok": true, "channel": channel})),
                )
                .expect(1)
                .mount(&server)
                .await;
            let mut after = channel;
            after["topic"]["value"] = json!("new 🦀");
            Mock::given(method("POST"))
                .and(path(format!("/api/{endpoint}")))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(json!({"ok": true, "channel": after})),
                )
                .expect(u64::from(!preview))
                .mount(&server)
                .await;
            let mut command = args.clone();
            command.extend([
                "--profile",
                "work",
                if preview { "--dry-run" } else { "--yes" },
            ]);
            let result = prepare(&command)
                .unwrap()
                .execute(&client(&server), &credentials)
                .await
                .unwrap();
            let mut text = Vec::new();
            result.write(false, &mut text).unwrap();
            let text = String::from_utf8(text).unwrap();
            assert!(!text.contains('\x1b'));
            assert!(!text.contains("hidden"));
            let value = serde_json::to_value(result).unwrap();
            if preview {
                assert_eq!(value["dry_run"], true);
                assert_eq!(value["before"]["channel_id"], args[2]);
                assert_eq!(value["user_id"], "W87654321");
            } else {
                assert_eq!(value["channel_id"], args[2]);
                let requests = server.received_requests().await.unwrap();
                assert_eq!(requests.last().unwrap().body_json::<Value>().unwrap(), body);
            }
            assert_eq!(
                server.received_requests().await.unwrap().len(),
                if preview { 2 } else { 3 }
            );
        }
    }
}

#[tokio::test]
async fn channel_creation_preserves_explicit_visibility_and_rejects_ambiguous_acknowledgements() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for visibility in ["public", "private"] {
        for outcome in [
            "preview",
            "success",
            "wrong_visibility",
            "wrong_name",
            "missing_scope",
            "unknown",
        ] {
            let server = MockServer::start().await;
            identity(&server).await;
            let is_private = visibility == "private";
            let response = match outcome {
                "missing_scope" => ResponseTemplate::new(200).set_body_json(json!({"ok": false, "error": "missing_scope"})),
                "unknown" => ResponseTemplate::new(503),
                _ => ResponseTemplate::new(200).set_body_json(json!({"ok": true, "channel": {"id": "C9", "name": if outcome == "wrong_name" { "different" } else { "release-ops" }, "is_channel": true, "is_private": if outcome == "wrong_visibility" { !is_private } else { is_private }}})),
            };
            Mock::given(path("/api/conversations.create"))
                .respond_with(response)
                .expect(u64::from(outcome != "preview"))
                .mount(&server)
                .await;
            let result = prepare(&[
                "channel",
                "create",
                "release-ops",
                "--visibility",
                visibility,
                "--profile",
                "work",
                if outcome == "preview" {
                    "--dry-run"
                } else {
                    "--yes"
                },
            ])
            .unwrap()
            .execute(&client(&server), &credentials)
            .await;
            match outcome {
                "preview" => assert_eq!(
                    serde_json::to_value(result.unwrap()).unwrap()["is_private"],
                    is_private
                ),
                "success" => assert_eq!(
                    serde_json::to_value(result.unwrap()).unwrap()["channel"]["channel_id"],
                    "C9"
                ),
                "missing_scope" => assert_eq!(
                    result.err().unwrap().report().slack_error,
                    Some("missing_scope")
                ),
                _ => assert_eq!(result.err().unwrap().report().code, "write_outcome_unknown"),
            }
            let requests = server.received_requests().await.unwrap();
            assert_eq!(requests.len(), if outcome == "preview" { 1 } else { 2 });
            if outcome != "preview" {
                assert_eq!(
                    requests[1].body_json::<Value>().unwrap(),
                    json!({"name": "release-ops", "is_private": is_private})
                );
            }
        }
    }
}

#[test]
fn group_membership_visibility_and_write_approval_are_explicit() {
    for args in [
        vec!["group-dm", "open", "U1", "U1", "--dry-run"],
        vec!["group-dm", "open", "U1", "W87654321", "--dry-run"],
        vec!["group-dm", "open", "U1", "--dry-run"],
        vec![
            "group-dm",
            "open",
            "U1",
            "U2",
            "U3",
            "U4",
            "U5",
            "U6",
            "U7",
            "U8",
            "U9",
            "--dry-run",
        ],
        vec![
            "channel",
            "create",
            "Name-With-Caps",
            "--visibility",
            "private",
            "--dry-run",
        ],
        vec!["channel", "create", "no-default-visibility", "--dry-run"],
    ] {
        assert!(prepare(&args).is_err(), "{args:?}");
    }
    for (topic, valid) in [
        ("🦀".repeat(250), true),
        ("🦀".repeat(251), false),
        (String::new(), true),
    ] {
        assert_eq!(
            prepare(&["channel", "topic", "C1", &topic, "--dry-run"]).is_ok(),
            valid
        );
    }
    for args in [
        vec!["channel", "join", "C1"],
        vec!["channel", "leave", "C1"],
        vec!["channel", "archive", "C1"],
        vec!["channel", "topic", "C1", "topic"],
        vec!["channel", "mark", "D1", "1700000000.000001"],
        vec!["channel", "create", "new", "--visibility", "private"],
        vec!["group-dm", "open", "U1", "U2"],
        vec!["group-dm", "resume", "G1"],
        vec!["group-dm", "close", "G1"],
    ] {
        for options in [vec![], vec!["--yes"], vec!["--profile", "work"]] {
            let mut command = args.clone();
            command.extend(options);
            assert!(prepare(&command).is_err(), "{command:?}");
        }
    }
}

#[tokio::test]
async fn group_dm_previews_inspect_members_and_never_create_or_send_until_approved() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for outcome in [
        "preview",
        "success",
        "bot",
        "wrong_kind",
        "identity",
        "missing_scope",
    ] {
        let server = MockServer::start().await;
        let mut auth = auth_response();
        if outcome == "identity" {
            auth["user_id"] = json!("U9");
        }
        Mock::given(path("/api/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(auth))
            .expect(1)
            .mount(&server)
            .await;
        for id in ["U1", "W2"] {
            Mock::given(path("/api/users.info")).and(query_param("user", id))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true, "user": {"id": id, "name": id, "is_bot": outcome == "bot" && id == "W2"}})))
                .expect(u64::from(outcome != "identity")).mount(&server).await;
        }
        let response = if outcome == "missing_scope" {
            json!({"ok": false, "error": "missing_scope"})
        } else {
            json!({"ok": true, "no_op": true, "already_open": true, "channel": {"id": "G5", "is_private": true, "is_mpim": outcome != "wrong_kind"}})
        };
        let writes = !matches!(outcome, "preview" | "bot" | "identity");
        Mock::given(path("/api/conversations.open"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .expect(u64::from(writes))
            .mount(&server)
            .await;
        let result = prepare(&[
            "group-dm",
            "open",
            "U1",
            "W2",
            "--profile",
            "work",
            if outcome == "preview" {
                "--dry-run"
            } else {
                "--yes"
            },
        ])
        .unwrap()
        .execute(&client(&server), &credentials)
        .await;
        match outcome {
            "preview" => {
                let value = serde_json::to_value(result.unwrap()).unwrap();
                assert_eq!(value["targets"][0]["user_id"], "U1");
                assert_eq!(value["targets"][1]["user_id"], "W2");
            }
            "success" => {
                let value = serde_json::to_value(result.unwrap()).unwrap();
                assert_eq!(value["requested_users"], json!(["U1", "W2"]));
                assert_eq!(value["channel"]["channel_id"], "G5");
                assert_eq!(value["already_open"], true);
            }
            _ => assert_eq!(
                result.err().unwrap().report().code,
                match outcome {
                    "bot" => "unsupported_dm_target",
                    "identity" => "identity_mismatch",
                    "missing_scope" => "slack_error",
                    _ => "write_outcome_unknown",
                }
            ),
        }
        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests.len(),
            if outcome == "identity" {
                1
            } else if writes {
                4
            } else {
                3
            }
        );
        if writes {
            assert_eq!(
                requests[3].body_json::<Value>().unwrap(),
                json!({"users": ["U1", "W2"], "return_im": true})
            );
        }
    }
}

#[tokio::test]
async fn member_pages_workspace_identity_and_wrong_conversation_types_keep_their_boundaries() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    let server = MockServer::start().await;
    identity(&server).await;
    Mock::given(method("GET")).and(path("/api/conversations.members")).and(query_param("channel", "G2"))
        .and(query_param("limit", "7")).and(query_param("cursor", "next"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true, "members": ["U1", "W3"], "response_metadata": {"next_cursor": "more"}})))
        .expect(1).mount(&server).await;
    let value = prepare(&["channel", "members", "G2", "--cursor", "next"])
        .unwrap()
        .execute(&client(&server), &credentials)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(value).unwrap(),
        json!({"channel_id": "G2", "data": ["U1", "W3"], "pagination": {"next_cursor": "more"}})
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 2);

    for team in ["T12345678", "T9"] {
        let server = MockServer::start().await;
        identity(&server).await;
        Mock::given(method("GET"))
            .and(path("/api/team.info"))
            .and(query_param("team", "T12345678"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"ok": true, "team": {"id": team, "name": "workspace", "domain": "team"}}),
            ))
            .expect(1)
            .mount(&server)
            .await;
        let result = prepare(&["workspace"])
            .unwrap()
            .execute(&client(&server), &credentials)
            .await;
        if team == "T9" {
            assert_eq!(result.err().unwrap().report().code, "invalid_response");
        } else {
            assert_eq!(serde_json::to_value(result.unwrap()).unwrap()["id"], team);
        }
    }
    for args in [
        vec!["channel", "join", "G2"],
        vec!["channel", "archive", "G2"],
        vec!["group-dm", "resume", "G2"],
    ] {
        let server = MockServer::start().await;
        identity(&server).await;
        Mock::given(path("/api/conversations.info"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"ok": true, "channel": {"id": "G2", "is_im": true}})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let mut args = args;
        args.extend(["--profile", "work", "--yes"]);
        let error = prepare(&args)
            .unwrap()
            .execute(&client(&server), &credentials)
            .await
            .err()
            .unwrap();
        assert_eq!(error.report().code, "invalid_arguments");
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }
}
