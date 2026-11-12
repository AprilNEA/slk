use crate::app::Request;
use crate::auth::{Credentials, Kind, ProfileView, Store};
use crate::cli::{Cli, Command};
use crate::error::Result;
use crate::testing::{self, auth_response, execute};
use clap::{CommandFactory, Parser};
use serde_json::{Value, json};
use std::fs;
use wiremock::matchers::{path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn store() -> Store {
    Store::memory("work", &json!({
        "kind": "user",
        "identity": {"workspace_url": "https://team.slack.com", "team_id": "T12345678", "user_id": "W87654321"},
        "credentials": {"token": "xoxp-test"},
        "settings": {"page_size": 7}
    }).to_string()).unwrap()
}

fn prepare(args: &[&str], store: &Store) -> Result<(ProfileView, Request)> {
    let cli = Cli::try_parse_from(std::iter::once("slk").chain(args.iter().copied())).unwrap();
    let Command::Slackcli(command) = cli.command else {
        panic!("expected a slackcli command");
    };
    command.prepare(store, cli.profile.as_deref(), cli.json)
}

#[test]
fn supported_grammar_rejects_conflicting_targets_and_unimplemented_options() {
    Cli::command().debug_assert();
    let link = "https://team.slack.com/archives/C1/p1700000000000002";
    for args in [
        vec!["messages", "send", "--recipient-id", "C1"],
        vec!["messages", "send", "--message", "body"],
        vec![
            "messages",
            "send",
            "--recipient-id",
            "C1",
            "--message",
            "body",
            "--message-file",
            "-",
        ],
        vec![
            "messages",
            "send",
            "--recipient-id",
            "C1",
            "--permalink",
            link,
            "--message",
            "body",
        ],
        vec![
            "messages",
            "send",
            "--permalink",
            link,
            "--thread-ts",
            "1700000000.000001",
            "--message",
            "body",
        ],
        vec![
            "messages",
            "send",
            "--recipient-id",
            "U1",
            "--message",
            "body",
        ],
        vec![
            "messages",
            "send",
            "--recipient-id",
            "C1",
            "--message",
            "body",
            "--file",
            "attachment.txt",
        ],
        vec!["messages", "send", "--users", "U1,U2", "--message", "body"],
        vec![
            "messages",
            "edit",
            "--channel-id",
            "C1",
            "--message",
            "body",
        ],
        vec!["messages", "delete", "--timestamp", "1700000000.000002"],
        vec!["messages", "react", "--permalink", link],
        vec![
            "messages",
            "react",
            "--permalink",
            link,
            "--channel-id",
            "C1",
            "--emoji",
            "wave",
        ],
        vec![
            "messages",
            "update",
            "--permalink",
            link,
            "--message",
            "body",
        ],
        vec!["conversations", "get", "C1"],
        vec!["conversations", "read"],
        vec!["conversations", "read", "C1", "--permalink", link],
        vec!["conversations", "members", "C1"],
        vec!["conversations", "info", "C1"],
        vec!["conversations", "list", "--types", "public_channel"],
        vec![
            "conversations",
            "list",
            "--workspace",
            "work",
            "--profile",
            "work",
        ],
        vec!["conversations", "list", "--fields", "name"],
        vec!["search", "messages", "deploy", "--sort", "score"],
        vec!["files", "read", "F1"],
    ] {
        assert!(
            Cli::try_parse_from(std::iter::once("slk").chain(args.iter().copied())).is_err(),
            "{args:?}"
        );
    }
    for args in [
        vec!["auth", "whoami", "--workspace", "work"],
        vec![
            "auth",
            "login-auto",
            "--workspace-url",
            "https://team.slack.com",
            "--profile",
            "work",
        ],
        vec!["users", "list", "--workspace", "work"],
        vec!["users", "info", "U1", "--workspace", "work"],
        vec!["files", "info", "F1", "--workspace", "work"],
        vec![
            "files",
            "download",
            "F1",
            "--output",
            "file.txt",
            "--workspace",
            "work",
        ],
        vec!["conversations", "members", "list", "C1"],
    ] {
        assert!(
            Cli::try_parse_from(std::iter::once("slk").chain(args.iter().copied())).is_ok(),
            "{args:?}"
        );
    }
}

#[tokio::test]
async fn message_adapters_preserve_exact_targets_verified_threads_and_body_files() {
    let server = MockServer::start().await;
    Mock::given(path("/api/auth.test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(auth_response()))
        .mount(&server)
        .await;
    let root = json!({"ts": "1700000000.000001", "text": "parent", "user": "W87654321"});
    let reply = json!({"ts": "1700000000.000009", "thread_ts": "1700000000.000001", "text": "reply", "user": "W87654321"});
    Mock::given(path("/api/conversations.history"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"ok": true, "messages": [root]})),
        )
        .mount(&server)
        .await;
    Mock::given(path("/api/conversations.replies"))
        .and(query_param("limit", "2"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok": true, "messages": [root, reply]})),
        )
        .mount(&server)
        .await;
    Mock::given(path("/api/conversations.replies"))
        .and(query_param("limit", "7"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok": true, "messages": [root, reply]})),
        )
        .mount(&server)
        .await;
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    let store = store();
    let link = "https://team.slack.com/archives/C1/p1700000000000009?thread_ts=1700000000.000001";
    let file = tempfile::NamedTempFile::new().unwrap();
    fs::write(file.path(), "literal $(never-run)\n").unwrap();
    for args in [
        vec![
            "messages",
            "send",
            "--recipient-id",
            "C1",
            "--message-file",
            file.path().to_str().unwrap(),
            "--dry-run",
        ],
        vec![
            "messages",
            "send",
            "--permalink",
            link,
            "--message",
            "new reply",
            "--dry-run",
        ],
        vec![
            "messages",
            "send",
            "--recipient-id",
            "C1",
            "--thread-ts",
            "p1700000000000001",
            "--message",
            "new reply",
            "--dry-run",
        ],
        vec![
            "messages",
            "edit",
            "--permalink",
            link,
            "--message",
            "new text",
            "--dry-run",
        ],
        vec!["messages", "delete", "--permalink", link, "--dry-run"],
        vec![
            "messages",
            "react",
            "--permalink",
            link,
            "--emoji",
            "wave",
            "--dry-run",
        ],
        vec![
            "messages",
            "unreact",
            "--permalink",
            link,
            "--emoji",
            "wave",
            "--dry-run",
        ],
        vec!["conversations", "get", "--permalink", link],
        vec!["conversations", "read", "--permalink", link],
        vec!["conversations", "get", "C1", "1700000000000001"],
    ] {
        let value = serde_json::to_value(
            execute(prepare(&args, &store).unwrap(), &server, &credentials)
                .await
                .unwrap(),
        )
        .unwrap();
        match (args[0], args[1]) {
            ("messages", "send") if args.contains(&"--message-file") => {
                assert_eq!(value["text"], "literal $(never-run)\n");
                assert!(value["thread_ts"].is_null());
            }
            ("messages", "send") => assert_eq!(value["thread_ts"], "1700000000.000001"),
            ("messages", _) => assert_eq!(value["message"]["ts"], "1700000000.000009"),
            (_, "get") if args.contains(&"--permalink") => {
                assert_eq!(value["ts"], "1700000000.000009")
            }
            (_, "get") => assert_eq!(value["ts"], "1700000000.000001"),
            (_, "read") => {
                assert_eq!(value["data"][0]["ts"], "1700000000.000001");
                assert_eq!(value["data"][1]["ts"], "1700000000.000009");
            }
            _ => unreachable!(),
        }
    }
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|request| matches!(
                request.url.path(),
                "/api/auth.test" | "/api/conversations.history" | "/api/conversations.replies"
            ))
    );
    Mock::given(path("/api/chat.postMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true, "channel": "C1", "ts": "1700000000.000010", "message": {"thread_ts": "1700000000.000001"}
        })))
        .expect(1).mount(&server).await;
    let result = execute(
        prepare(
            &[
                "messages",
                "send",
                "--permalink",
                link,
                "--message",
                "approved reply",
                "--workspace",
                "T12345678",
                "--yes",
                "--json",
            ],
            &store,
        )
        .unwrap(),
        &server,
        &credentials,
    )
    .await
    .unwrap();
    let result = serde_json::to_value(result).unwrap();
    assert_eq!(result["ts"], "1700000000.000010");
    let posted = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|request| request.url.path() == "/api/chat.postMessage")
        .unwrap()
        .body_json::<Value>()
        .unwrap();
    assert_eq!(posted["channel"], "C1");
    assert_eq!(posted["thread_ts"], "1700000000.000001");
    assert_eq!(posted["text"], "approved reply");
}

#[test]
fn compatibility_does_not_approve_writes_or_choose_ambiguous_workspaces() {
    let store = store();
    for approval in [
        vec![],
        vec!["--yes"],
        vec!["--workspace", "work"],
        vec!["--profile", "work"],
    ] {
        let mut args = vec![
            "messages",
            "send",
            "--recipient-id",
            "C1",
            "--message",
            "body",
            "--json",
        ];
        args.extend(approval);
        assert!(prepare(&args, &store).is_err());
    }
    let args = [
        "messages",
        "send",
        "--recipient-id",
        "C1",
        "--message",
        "body",
        "--json",
        "--workspace",
        "T12345678",
        "--yes",
    ];
    assert!(prepare(&args, &store).is_ok());
    let mismatched = [
        "messages",
        "send",
        "--permalink",
        "https://other.slack.com/archives/C1/p1700000000000001",
        "--message",
        "body",
        "--dry-run",
    ];
    let error = prepare(&mismatched, &store).err().unwrap();
    assert_eq!(error.report().code, "workspace_mismatch");

    let (_directory, store) = testing::store(&["one", "two"], Kind::User, json!({}));
    assert!(
        prepare(
            &["conversations", "list", "--workspace", "T12345678"],
            &store
        )
        .is_err()
    );
    assert!(prepare(&["conversations", "list", "--workspace", "missing"], &store).is_err());
    assert!(prepare(&["conversations", "list", "--workspace", "one"], &store).is_ok());
}

#[tokio::test]
async fn compatibility_history_is_chronological_and_search_keeps_native_queries() {
    let server = MockServer::start().await;
    Mock::given(path("/api/auth.test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(auth_response()))
        .mount(&server)
        .await;
    Mock::given(path("/api/conversations.history"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true, "messages": [{"ts": "1700000000.000009"}, {"ts": "1700000000.000002"}],
            "response_metadata": {"next_cursor": "NEXT"}
        })))
        .mount(&server)
        .await;
    Mock::given(path("/api/assistant.search.context"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok": true, "results": {"messages": []}})),
        )
        .mount(&server)
        .await;
    let store = store();
    let credentials = store.credentials("work").unwrap();
    let value = serde_json::to_value(
        execute(
            prepare(&["conversations", "read", "C1"], &store).unwrap(),
            &server,
            &credentials,
        )
        .await
        .unwrap(),
    )
    .unwrap();
    assert_eq!(value["data"][0]["ts"], "1700000000.000002");
    assert_eq!(value["data"][1]["ts"], "1700000000.000009");
    assert_eq!(value["pagination"]["next_cursor"], "NEXT");
    for args in [
        vec!["search", "messages", "deploy in:ops"],
        vec!["search", "messages"],
    ] {
        let cli = Cli::try_parse_from(std::iter::once("slk").chain(args.iter().copied())).unwrap();
        let Command::Messages(command) = cli.command else {
            panic!("expected search")
        };
        execute(
            command.prepare(&store, None, true).unwrap(),
            &server,
            &credentials,
        )
        .await
        .unwrap();
    }
    let queries = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.url.path() == "/api/assistant.search.context")
        .map(|request| {
            request.body_json::<Value>().unwrap()["query"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(queries, ["deploy in:ops", "messages"]);
}
