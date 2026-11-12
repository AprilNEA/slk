use crate::app::Request;
use crate::auth::{Credentials, Kind, ProfileView};
use crate::cli::{Cli, Command};
use crate::error::Result;
use crate::model::scheduled::validate_time;
use crate::testing::{self, auth_response, execute};
use chrono::{DateTime, Duration, Timelike, Utc};
use clap::Parser;
use serde_json::{Value, json};
use std::fs;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn prepare(args: &[&str]) -> Result<(ProfileView, Request)> {
    let (_directory, store) = testing::store(&["work"], Kind::User, json!({"page_size": 7}));
    let cli = Cli::try_parse_from(std::iter::once("slk").chain(args.iter().copied())).unwrap();
    let Command::Schedule { command } = cli.command else {
        panic!("expected schedule")
    };
    command.prepare(&store, cli.profile.as_deref(), cli.json)
}

#[test]
fn scheduling_uses_absolute_time_and_rejects_past_fractional_or_too_distant_dates() {
    let now: DateTime<Utc> = "2026-01-01T00:00:00Z".parse().unwrap();
    for (time, valid) in [
        ("2025-12-31T23:59:59Z", false),
        ("2026-01-01T00:00:00Z", false),
        ("2026-01-01T00:00:00.5Z", false),
        ("2026-01-01T05:30:01+05:30", true),
        ("2026-05-01T00:00:00Z", true),
        ("2026-05-01T00:00:01Z", false),
    ] {
        assert_eq!(
            validate_time(time.parse().unwrap(), now).is_ok(),
            valid,
            "{time}"
        );
    }
    assert_eq!(
        validate_time("2026-01-01T05:30:01+05:30".parse().unwrap(), now).unwrap(),
        1_767_225_601
    );
    assert!(
        Cli::try_parse_from([
            "slk",
            "schedule",
            "create",
            "C1",
            "--at",
            "2026-01-02T01:00:00",
            "--text",
            "body",
            "--dry-run"
        ])
        .is_err()
    );
}

#[tokio::test]
async fn scheduled_replies_share_thread_validation_and_write_safety_with_immediate_messages() {
    let at = (Utc::now() + Duration::hours(2))
        .with_nanosecond(0)
        .unwrap();
    let local_time = at
        .with_timezone(&chrono::FixedOffset::east_opt(19_800).unwrap())
        .to_rfc3339();
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for (command, destination, thread) in [
        ("create", "C1", None),
        (
            "reply",
            "https://team.slack.com/archives/C1/p1700000000000001",
            Some("1700000000.000001"),
        ),
    ] {
        for preview in [true, false] {
            let server = MockServer::start().await;
            Mock::given(path("/api/auth.test"))
                .respond_with(ResponseTemplate::new(200).set_body_json(auth_response()))
                .mount(&server)
                .await;
            if thread.is_some() {
                Mock::given(path("/api/conversations.history"))
                    .respond_with(
                        ResponseTemplate::new(200)
                            .set_body_json(json!({"ok": true, "messages": [{"ts": thread}]})),
                    )
                    .mount(&server)
                    .await;
            }
            Mock::given(method("POST")).and(path("/api/chat.scheduleMessage"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true, "channel": "C1", "scheduled_message_id": "Q24", "post_at": at.timestamp().to_string()})))
                .expect(u64::from(!preview)).mount(&server).await;
            let mut args = vec![
                "schedule",
                command,
                destination,
                "--at",
                &local_time,
                "--text",
                "asymmetric 🦀",
                "--profile",
                "work",
                "--json",
            ];
            args.push(if preview { "--dry-run" } else { "--yes" });
            let output = execute(prepare(&args).unwrap(), &server, &credentials)
                .await
                .unwrap();
            let result = serde_json::to_value(output).unwrap();
            assert_eq!(result["channel_id"], "C1");
            assert_eq!(result["post_at"], at.timestamp());
            if preview {
                assert_eq!(result["dry_run"], true);
                assert_eq!(result["thread_ts"], json!(thread));
            } else {
                assert_eq!(result["scheduled_message_id"], "Q24");
                let requests = server.received_requests().await.unwrap();
                let writes: Vec<_> = requests
                    .iter()
                    .filter(|request| request.url.path() == "/api/chat.scheduleMessage")
                    .collect();
                assert_eq!(writes.len(), 1);
                let body = writes[0].body_json::<Value>().unwrap();
                assert_eq!(body["post_at"], at.timestamp());
                assert_eq!(body["thread_ts"], json!(thread));
                assert_eq!(body["text"], "asymmetric 🦀");
                for flag in [
                    "reply_broadcast",
                    "unfurl_links",
                    "unfurl_media",
                    "link_names",
                ] {
                    assert_eq!(body[flag], false);
                }
                assert_eq!(body["parse"], "none");
                assert!(body.get("metadata").is_none());
            }
        }
    }
    assert!(
        prepare(&[
            "schedule",
            "create",
            "C1",
            "--at",
            &local_time,
            "--text",
            "body",
            "--yes",
            "--json"
        ])
        .is_err()
    );
}

#[tokio::test]
async fn scheduled_listing_and_cancellation_use_one_page_and_the_exact_returned_id() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for action in ["list", "preview", "cancel", "missing", "wrong_channel"] {
        let server = MockServer::start().await;
        Mock::given(path("/api/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(auth_response()))
            .mount(&server)
            .await;
        Mock::given(method("POST")).and(path("/api/chat.scheduledMessages.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true, "scheduled_messages": [
                {"id": 24, "channel_id": if action == "wrong_channel" {"C9"} else {"C1"}, "post_at": 1794000000, "text": "visible\u{001b}]52;discarded"}
            ], "response_metadata": {"next_cursor": "later"}}))).expect(1).mount(&server).await;
        Mock::given(method("POST"))
            .and(path("/api/chat.deleteScheduledMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .expect(u64::from(action == "cancel"))
            .mount(&server)
            .await;
        let mut args = if action == "list" {
            vec!["schedule", "list", "C1"]
        } else {
            vec![
                "schedule",
                "cancel",
                "C1",
                if action == "missing" { "Q29" } else { "24" },
            ]
        };
        args.extend(["--cursor", "next+/=", "--profile", "work"]);
        if action != "list" {
            args.push(if action == "preview" {
                "--dry-run"
            } else {
                "--yes"
            });
        }
        let result = execute(prepare(&args).unwrap(), &server, &credentials).await;
        match action {
            "missing" => assert_eq!(
                result.err().unwrap().report().code,
                "scheduled_message_not_found"
            ),
            "wrong_channel" => assert_eq!(result.err().unwrap().report().code, "invalid_response"),
            _ => {
                let output = result.unwrap();
                let mut text = Vec::new();
                output.write(false, &mut text).unwrap();
                let text = String::from_utf8(text).unwrap();
                assert!(!text.contains('\x1b'));
                assert!(!text.contains("discarded"));
                let json = serde_json::to_value(output).unwrap();
                if action == "list" {
                    assert_eq!(json["pagination"]["next_cursor"], "later");
                    assert_eq!(json["data"][0]["scheduled_message_id"], "24");
                }
                if action == "preview" {
                    assert_eq!(json["scheduled"]["scheduled_message_id"], "24");
                    assert_eq!(json["dry_run"], true);
                }
                if action == "cancel" {
                    assert_eq!(json["cancelled"]["scheduled_message_id"], "24");
                }
            }
        }
        for request in server.received_requests().await.unwrap() {
            let body = request.body_json::<Value>().unwrap();
            if request.url.path() == "/api/chat.scheduledMessages.list" {
                assert_eq!(
                    body,
                    json!({"channel": "C1", "cursor": "next+/=", "limit": 7})
                );
            }
            if request.url.path() == "/api/chat.deleteScheduledMessage" {
                assert_eq!(body, json!({"channel": "C1", "scheduled_message_id": "24"}));
            }
        }
    }
}

#[tokio::test]
async fn uncertain_schedules_do_not_retry_and_identity_changes_prevent_data_requests() {
    let at = (Utc::now() + Duration::hours(2))
        .with_nanosecond(0)
        .unwrap();
    let time = at.to_rfc3339();
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for outcome in [
        "identity",
        "wrong_channel",
        "wrong_time",
        "disconnect",
        "missing_scope",
    ] {
        let server = MockServer::start().await;
        let mut identity = auth_response();
        if outcome == "identity" {
            identity["team_id"] = json!("T9999");
        }
        Mock::given(path("/api/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(identity))
            .mount(&server)
            .await;
        let response = match outcome {
            "disconnect" => ResponseTemplate::new(503),
            "missing_scope" => ResponseTemplate::new(200).set_body_json(json!({"ok": false, "error": "missing_scope"})),
            _ => ResponseTemplate::new(200).set_body_json(json!({"ok": true, "channel": if outcome == "wrong_channel" {"C9"} else {"C1"}, "scheduled_message_id": "Q24", "post_at": at.timestamp() + i64::from(outcome == "wrong_time")})),
        };
        Mock::given(path("/api/chat.scheduleMessage"))
            .respond_with(response)
            .expect(u64::from(outcome != "identity"))
            .mount(&server)
            .await;
        let error = execute(
            prepare(&[
                "schedule",
                "create",
                "C1",
                "--at",
                &time,
                "--text",
                "body",
                "--profile",
                "work",
                "--yes",
            ])
            .unwrap(),
            &server,
            &credentials,
        )
        .await
        .err()
        .unwrap();
        assert_eq!(
            error.report().code,
            match outcome {
                "identity" => "identity_mismatch",
                "missing_scope" => "slack_error",
                _ => "write_outcome_unknown",
            }
        );
    }
}

#[tokio::test]
async fn scheduled_block_kit_messages_preview_and_send_the_same_complete_payload() {
    let temporary = tempfile::tempdir().unwrap();
    let file = temporary.path().join("blocks.json");
    let blocks = json!([{"type": "header", "text": {"type": "plain_text", "text": "Deployment"}, "future": true}]);
    fs::write(&file, blocks.to_string()).unwrap();
    let at = (Utc::now() + Duration::hours(2))
        .with_nanosecond(0)
        .unwrap();
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for preview in [true, false] {
        let server = MockServer::start().await;
        Mock::given(path("/api/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(auth_response()))
            .mount(&server)
            .await;
        Mock::given(path("/api/chat.scheduleMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true, "channel": "C9", "post_at": at.timestamp(), "scheduled_message_id": "Q7"
            })))
            .expect(u64::from(!preview))
            .mount(&server)
            .await;
        let output = execute(
            prepare(&[
                "schedule",
                "create",
                "C9",
                "--at",
                &at.to_rfc3339(),
                "--text",
                "Deployment at the selected time",
                "--blocks-file",
                file.to_str().unwrap(),
                "--profile",
                "work",
                if preview { "--dry-run" } else { "--yes" },
            ])
            .unwrap(),
            &server,
            &credentials,
        )
        .await
        .unwrap();
        if preview {
            let output = serde_json::to_value(output).unwrap();
            assert_eq!(output["blocks"], blocks);
            assert_eq!(output["post_at"], at.timestamp());
        } else {
            let requests = server.received_requests().await.unwrap();
            let request = requests
                .iter()
                .find(|request| request.url.path() == "/api/chat.scheduleMessage")
                .unwrap();
            let body = request.body_json::<Value>().unwrap();
            assert_eq!(body["blocks"], blocks);
            assert_eq!(body["post_at"], at.timestamp());
            assert_eq!(body["text"], "Deployment at the selected time");
            assert_eq!(body["channel"], "C9");
        }
    }
}
