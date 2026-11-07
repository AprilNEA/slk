use super::Prepared;
use crate::auth::{Credentials, Kind, Store};
use crate::cli::{Cli, Command};
use crate::error::Result;
use crate::slack::tests::{auth_response, client};
use clap::Parser;
use serde_json::{Value, json};
use std::fs;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod blocks;
mod modify;
mod split;

fn prepare(args: &[&str], kind: Kind) -> Result<Prepared> {
    let temporary = tempfile::tempdir().unwrap();
    fs::write(temporary.path().join("config.json"), json!({
        "settings": {"page_size": 7},
        "profiles": {"work": {
            "kind": kind,
            "identity": {"workspace_url": "https://team.slack.com/", "team_id": "T12345678", "user_id": "W87654321", "bot_id": (kind == Kind::Bot).then_some("B7")},
            "keyring_ref": "00112233-4455-4677-8899-aabbccddeeff"
        }}
    }).to_string()).unwrap();
    let store = Store::load(temporary.path().to_path_buf()).unwrap();
    let cli = Cli::try_parse_from(std::iter::once("slk").chain(args.iter().copied())).unwrap();
    let Command::Messages(command) = cli.command else {
        panic!("expected a message command");
    };
    command.prepare(&store, cli.profile.as_deref(), cli.json)
}

#[tokio::test]
async fn dry_runs_return_verified_identity_and_body_without_writes() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for (args, expected_thread) in [
        (
            vec![
                "send",
                "C1",
                "--text",
                "literal $(never-run)",
                "--dry-run",
                "--json",
            ],
            Value::Null,
        ),
        (
            vec![
                "reply",
                "https://team.slack.com/archives/C1/p1700000000000001",
                "--text",
                "literal $(never-run)",
                "--dry-run",
                "--json",
            ],
            json!("1700000000.000001"),
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(path("/api/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(auth_response()))
            .expect(1)
            .mount(&server)
            .await;
        if !expected_thread.is_null() {
            Mock::given(path("/api/conversations.history"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(
                        json!({"ok": true, "messages": [{"ts": "1700000000.000001"}]}),
                    ),
                )
                .expect(1)
                .mount(&server)
                .await;
        }
        let result = prepare(&args, Kind::User)
            .unwrap()
            .execute(&client(&server), &credentials)
            .await
            .unwrap();
        let result = serde_json::to_value(result).unwrap();
        assert_eq!(result["dry_run"], true);
        assert_eq!(result["profile"], "work");
        assert_eq!(result["user_id"], "W87654321");
        assert_eq!(result["channel_id"], "C1");
        assert_eq!(result["thread_ts"], expected_thread);
        assert_eq!(result["text"], "literal $(never-run)");
        assert!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .all(|request| request.url.path() != "/api/chat.postMessage")
        );
    }
}

#[tokio::test]
async fn non_threadable_roots_are_rejected_before_preview_or_post() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    let root_url = "https://team.slack.com/archives/C1/p1700000000000001";
    let broadcast_url = "https://team.slack.com/archives/C1/p1700000000000002";
    for subtype in ["channel_join", "channel_leave"] {
        for link in [root_url, broadcast_url] {
            for (command, options) in [
                ("reply", &["--text", "body", "--dry-run"][..]),
                (
                    "reply",
                    &["--text", "body", "--profile", "work", "--yes"][..],
                ),
                ("thread", &[][..]),
            ] {
                let server = MockServer::start().await;
                Mock::given(path("/api/auth.test"))
                    .respond_with(ResponseTemplate::new(200).set_body_json(auth_response()))
                    .mount(&server)
                    .await;
                Mock::given(path("/api/conversations.history"))
                    .and(query_param("latest", "1700000000.000001"))
                    .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                        "ok": true,
                        "messages": [{"ts": "1700000000.000001", "subtype": subtype}]
                    })))
                    .mount(&server)
                    .await;
                if link == broadcast_url {
                    Mock::given(path("/api/conversations.history"))
                        .and(query_param("latest", "1700000000.000002"))
                        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                            "ok": true,
                            "messages": [{
                                "ts": "1700000000.000002",
                                "thread_ts": "1700000000.000001",
                                "subtype": "thread_broadcast"
                            }]
                        })))
                        .mount(&server)
                        .await;
                }
                let mut args = vec![command, link];
                args.extend_from_slice(options);
                let error = match prepare(&args, Kind::User)
                    .unwrap()
                    .execute(&client(&server), &credentials)
                    .await
                {
                    Ok(_) => panic!("{subtype} must not be accepted by {args:?}"),
                    Err(error) => error,
                };
                assert_eq!(error.report().code, "thread_unresolved", "{args:?}");
                assert!(
                    server
                        .received_requests()
                        .await
                        .unwrap()
                        .iter()
                        .all(|request| !matches!(
                            request.url.path(),
                            "/api/chat.postMessage" | "/api/conversations.replies"
                        ))
                );
            }
        }
    }
}

#[test]
fn automation_requires_both_flags_and_links_must_match_the_profile() {
    for args in [
        vec!["send", "C1", "--text", "body", "--json"],
        vec!["send", "C1", "--text", "body", "--yes", "--json"],
        vec![
            "send",
            "C1",
            "--text",
            "body",
            "--profile",
            "work",
            "--json",
        ],
    ] {
        let error = match prepare(&args, Kind::User) {
            Ok(_) => panic!("write must require explicit consent"),
            Err(error) => error,
        };
        assert_eq!(error.report().code, "invalid_arguments");
    }
    for command in [
        "thread", "reply", "edit", "delete", "react", "unreact", "pin", "unpin",
    ] {
        let mut args = vec![
            command,
            "https://other.slack.com/archives/C1/p1700000000000001",
            "--profile",
            "work",
        ];
        if matches!(command, "reply" | "edit") {
            args.extend(["--text", "body", "--yes"]);
        } else if matches!(command, "react" | "unreact") {
            args.extend(["wave", "--yes"]);
        }
        let error = match prepare(&args, Kind::User) {
            Ok(_) => panic!("cross-workspace links must fail"),
            Err(error) => error,
        };
        assert_eq!(error.report().code, "workspace_mismatch");
    }
}

#[tokio::test]
async fn identity_changes_abort_before_the_write_api() {
    let server = MockServer::start().await;
    let mut response = auth_response();
    response["user_id"] = json!("U9999");
    Mock::given(path("/api/auth.test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response))
        .expect(1)
        .mount(&server)
        .await;
    let prepared = prepare(
        &[
            "send",
            "C1",
            "--text",
            "body",
            "--profile",
            "work",
            "--yes",
            "--json",
        ],
        Kind::User,
    )
    .unwrap();
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    let error = match prepared.execute(&client(&server), &credentials).await {
        Ok(_) => panic!("identity change must fail"),
        Err(error) => error,
    };
    assert_eq!(error.report().code, "identity_mismatch");
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_file_reply_uses_the_confirmed_root_and_returns_the_acknowledgement() {
    let server = MockServer::start().await;
    Mock::given(path("/api/auth.test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(auth_response()))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/api/conversations.history"))
        .and(query_param("latest", "1700000000.000001"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok": true, "messages": [{"ts": "1700000000.000001"}]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/api/conversations.replies"))
        .and(query_param("ts", "1700000000.000001"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true, "messages": [{"ts": "1700000000.000002", "thread_ts": "1700000000.000001"}]})))
        .expect(1).mount(&server).await;
    Mock::given(path("/api/chat.postMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true, "channel": "C1", "ts": "1700000000.000009", "message": {"thread_ts": "1700000000.000001"}})))
        .expect(1).mount(&server).await;
    let file = tempfile::NamedTempFile::new().unwrap();
    let body = "first line\n$(never-run) 🦀\n";
    fs::write(file.path(), body).unwrap();
    let prepared = prepare(
        &[
            "reply",
            "https://team.slack.com/archives/C1/p1700000000000002?thread_ts=1700000000.000001",
            "--file",
            file.path().to_str().unwrap(),
            "--profile",
            "work",
            "--yes",
            "--json",
        ],
        Kind::User,
    )
    .unwrap();
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    let result = prepared
        .execute(&client(&server), &credentials)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        json!({"channel_id": "C1", "ts": "1700000000.000009", "thread_ts": "1700000000.000001"})
    );
    let requests = server.received_requests().await.unwrap();
    let writes: Vec<_> = requests
        .iter()
        .filter(|request| request.url.path() == "/api/chat.postMessage")
        .collect();
    assert_eq!(writes.len(), 1);
    assert_eq!(
        writes[0].body_json::<Value>().unwrap()["thread_ts"],
        "1700000000.000001"
    );
    assert_eq!(writes[0].body_json::<Value>().unwrap()["text"], body);
    assert_eq!(requests.len(), 4);
}

#[tokio::test]
async fn history_dispatch_uses_the_configured_page_size_and_cursor() {
    let server = MockServer::start().await;
    Mock::given(path("/api/auth.test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(auth_response()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/conversations.history"))
        .and(query_param("limit", "7"))
        .and(query_param("cursor", "next+/="))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"ok": true, "messages": [], "response_metadata": {"next_cursor": "later"}}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    let result = prepare(&["read", "C1", "--cursor", "next+/="], Kind::User)
        .unwrap()
        .execute(&client(&server), &credentials)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        json!({"data": [], "pagination": {"next_cursor": "later"}})
    );
}

#[tokio::test]
async fn rejected_sessions_require_explicit_login_without_retry_or_fallback() {
    for credentials in [
        Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap(),
        Credentials::parse(
            Kind::Browser,
            r#"{"token":"xoxc-test","cookie":"xoxd-test","workspace_url":"https://team.slack.com"}"#,
        )
        .unwrap(),
    ] {
        for code in ["invalid_auth", "not_authed", "token_revoked", "token_expired"] {
            let server = MockServer::start().await;
            Mock::given(path("/api/auth.test"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(json!({"ok": false, "error": code})),
                )
                .expect(1)
                .mount(&server)
                .await;
            let error = prepare(&["channels"], credentials.kind())
                .unwrap()
                .execute(&client(&server), &credentials)
                .await
                .err()
                .unwrap();
            let report = error.report();
            assert_eq!(report.code, "slack_error");
            assert_eq!(report.slack_error, Some(code));
            assert!(report.message.contains("auth login"));
            assert_eq!(server.received_requests().await.unwrap().len(), 1);
        }
    }
}
