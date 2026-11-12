use super::Prepared;
use crate::app::Connection;
use crate::auth::{Credentials, Kind, ProfileView};
use crate::cli::{Cli, Command, write::Terminal};
use crate::error::Result;
use crate::output::Output;
use crate::testing::{self, auth_response, client};
use clap::Parser;
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn prepare(args: &[&str], kind: Kind) -> Result<(ProfileView, Prepared)> {
    let (_directory, store) = testing::store(&["work"], kind, json!({"page_size": 7}));
    let cli = Cli::try_parse_from(std::iter::once("slk").chain(args.iter().copied())).unwrap();
    let Command::Users(command) = cli.command else {
        panic!("expected a user command");
    };
    command.prepare(&store, cli.profile.as_deref(), cli.json)
}

async fn execute(
    (profile, prepared): (ProfileView, Prepared),
    server: &MockServer,
    credentials: &Credentials,
) -> Result<Output> {
    let connection = Connection::verify(client(server), credentials.clone(), profile).await?;
    prepared.execute(connection, &Terminal).await
}

async fn identity(server: &MockServer) {
    Mock::given(path("/api/auth.test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(auth_response()))
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test]
async fn user_queries_preserve_directory_data_and_pagination() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    let user = json!({
        "id": "U1", "team_id": "T9999", "name": "alice", "deleted": true,
        "profile": {"display_name": "Alice", "email": "alice+test@example.test", "huddle_state": "future_state"},
        "future_field": {"anything": [true]}
    });
    for (args, endpoint, query, response) in [
        (
            vec!["users", "list", "--cursor", "next+/="],
            "/api/users.list",
            ("cursor", "next+/="),
            json!({"ok": true, "members": [user], "response_metadata": {"next_cursor": "later"}}),
        ),
        (
            vec!["users", "info", "U1"],
            "/api/users.info",
            ("user", "U1"),
            json!({"ok": true, "user": user}),
        ),
        (
            vec!["users", "lookup", "alice+test@example.test"],
            "/api/users.lookupByEmail",
            ("email", "alice+test@example.test"),
            json!({"ok": true, "user": user}),
        ),
    ] {
        let server = MockServer::start().await;
        identity(&server).await;
        Mock::given(method("GET"))
            .and(path(endpoint))
            .and(query_param(query.0, query.1))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .expect(1)
            .mount(&server)
            .await;
        let result = execute(prepare(&args, Kind::User).unwrap(), &server, &credentials)
            .await
            .unwrap();
        let json = serde_json::to_value(result).unwrap();
        let item = if endpoint == "/api/users.list" {
            assert_eq!(json["pagination"], json!({"next_cursor": "later"}));
            let requests = server.received_requests().await.unwrap();
            assert!(requests.iter().any(|request| {
                request
                    .url
                    .query_pairs()
                    .any(|(key, value)| key == "limit" && value == "7")
            }));
            &json["data"][0]
        } else {
            &json
        };
        assert_eq!(item["user_id"], "U1");
        assert_eq!(item["team_id"], "T9999");
        assert_eq!(item["profile"]["display_name"], "Alice");
        assert_eq!(item["profile"]["email"], "alice+test@example.test");
        assert_eq!(item["deleted"], true);
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }
}

#[tokio::test]
async fn name_resolution_does_not_guess_across_ambiguous_or_incomplete_pages() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for kind in ["user", "channel"] {
        for (count, cursor, has_more, input_cursor, expected) in [
            (1, "", false, None, "unique"),
            (0, "", false, None, "not_found"),
            (2, "", false, None, "ambiguous"),
            (1, "next", false, None, "incomplete"),
            (1, "", true, None, "incomplete"),
            (0, "next", true, None, "incomplete"),
            (1, "", false, Some("previous"), "incomplete"),
        ] {
            let server = MockServer::start().await;
            identity(&server).await;
            let (endpoint, key, name, values) = if kind == "user" {
                (
                    "/api/users.list",
                    "members",
                    "@ALICE",
                    vec![
                        json!({"id": "U1", "name": "different", "profile": {"display_name": "Alice"}}),
                        json!({"id": "U2", "name": "other", "real_name": "alice"}),
                    ],
                )
            } else {
                (
                    "/api/conversations.list",
                    "channels",
                    "#GENERAL",
                    vec![
                        json!({"id": "C1", "name": "general", "is_channel": true}),
                        json!({"id": "C2", "name": "general", "is_channel": true, "is_private": true}),
                    ],
                )
            };
            let mut body = json!({"ok": true, "response_metadata": {"next_cursor": cursor}, "has_more": has_more});
            let mut data = values[..count].to_vec();
            data.push(json!({"id": "C9", "name": "nonmatch", "is_channel": true}));
            body[key] = json!(data);
            Mock::given(path(endpoint))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .expect(1)
                .mount(&server)
                .await;
            let mut args = vec!["resolve", kind, name];
            if let Some(cursor) = input_cursor {
                args.extend(["--cursor", cursor]);
            }
            let result = execute(prepare(&args, Kind::User).unwrap(), &server, &credentials)
                .await
                .unwrap();
            let result = serde_json::to_value(result).unwrap();
            assert_eq!(result["status"], expected, "{args:?}, {cursor}, {has_more}");
            assert_eq!(result["data"].as_array().unwrap().len(), count);
            assert_eq!(server.received_requests().await.unwrap().len(), 2);
        }
    }
}

#[tokio::test]
async fn dm_preview_never_opens_a_conversation_and_creation_never_sends() {
    for (kind, input) in [
        (Kind::User, r#"{"token":"xoxp-test"}"#),
        (
            Kind::Browser,
            r#"{"token":"xoxc-test","cookie":"xoxd-test","workspace_url":"https://team.slack.com/"}"#,
        ),
    ] {
        let credentials = Credentials::parse(kind, input).unwrap();
        for (flags, already_open) in [
            (vec!["--dry-run"], None),
            (vec!["--dry-run", "--yes"], None),
            (vec!["--profile", "work", "--yes"], Some(true)),
            (vec!["--profile", "work", "--yes"], Some(false)),
            (vec!["--profile", "work", "--yes"], None),
        ] {
            let server = MockServer::start().await;
            identity(&server).await;
            Mock::given(path("/api/users.info"))
                .and(query_param("user", "U1"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(json!({"ok": true, "user": {"id": "U1", "name": "alice"}})),
                )
                .expect(1)
                .mount(&server)
                .await;
            let dry_run = flags.contains(&"--dry-run");
            Mock::given(method("POST")).and(path("/api/conversations.open"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true, "channel": {"id": "D2", "user": "U1", "is_im": true}, "already_open": already_open, "no_op": already_open})))
                .expect(if dry_run { 0 } else { 1 }).mount(&server).await;
            let mut args = vec!["dm", "U1", "--json"];
            args.extend(flags);
            let output = execute(prepare(&args, kind).unwrap(), &server, &credentials)
                .await
                .unwrap();
            let result = serde_json::to_value(output).unwrap();
            if dry_run {
                assert_eq!(result["dry_run"], true);
                assert_eq!(result["profile"], "work");
                assert_eq!(result["user_id"], "W87654321");
                assert_eq!(result["target"]["user_id"], "U1");
            } else {
                assert_eq!(
                    result,
                    json!({"channel_id": "D2", "user_id": "U1", "already_open": already_open, "no_op": already_open})
                );
            }
            let requests = server.received_requests().await.unwrap();
            assert_eq!(requests.len(), if dry_run { 2 } else { 3 });
            for request in requests
                .iter()
                .filter(|request| request.url.path() == "/api/conversations.open")
            {
                assert_eq!(
                    request.body_json::<Value>().unwrap(),
                    json!({"users": ["U1"], "return_im": true})
                );
            }
        }
    }
}

#[tokio::test]
async fn invalid_dm_targets_and_api_rejections_do_not_mutate() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for (body, expected, slack_error) in [
        (
            json!({"ok": true, "user": {"id": "U1", "deleted": true}}),
            "unsupported_dm_target",
            None,
        ),
        (
            json!({"ok": true, "user": {"id": "U1", "is_bot": true}}),
            "unsupported_dm_target",
            None,
        ),
        (
            json!({"ok": true, "user": {"id": "U1", "is_app_user": true}}),
            "unsupported_dm_target",
            None,
        ),
        (
            json!({"ok": true, "user": {"id": "U9"}}),
            "invalid_response",
            None,
        ),
        (
            json!({"ok": false, "error": "user_not_found"}),
            "slack_error",
            Some("user_not_found"),
        ),
        (
            json!({"ok": false, "error": "missing_scope"}),
            "slack_error",
            Some("missing_scope"),
        ),
    ] {
        let server = MockServer::start().await;
        identity(&server).await;
        Mock::given(path("/api/users.info"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;
        let error = execute(
            prepare(&["dm", "U1", "--profile", "work", "--yes"], Kind::User).unwrap(),
            &server,
            &credentials,
        )
        .await
        .err()
        .unwrap();
        assert_eq!(error.report().code, expected);
        assert_eq!(error.report().slack_error, slack_error);
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }
}

#[tokio::test]
async fn dm_acknowledgements_distinguish_unknown_outcomes_from_explicit_rejections() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for (response, expected) in [
        (ResponseTemplate::new(503), "write_outcome_unknown"),
        (
            ResponseTemplate::new(200).set_body_string("xoxp-test invalid JSON"),
            "write_outcome_unknown",
        ),
        (
            ResponseTemplate::new(200).set_body_json(
                json!({"ok": true, "channel": {"id": "D2", "user": "U9", "is_im": true}}),
            ),
            "write_outcome_unknown",
        ),
        (
            ResponseTemplate::new(200).set_body_json(
                json!({"ok": true, "channel": {"id": "C2", "user": "U1", "is_im": false}}),
            ),
            "write_outcome_unknown",
        ),
        (
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok": false, "error": "missing_scope"})),
            "slack_error",
        ),
    ] {
        let server = MockServer::start().await;
        identity(&server).await;
        Mock::given(path("/api/users.info"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"ok": true, "user": {"id": "U1"}})),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(path("/api/conversations.open"))
            .respond_with(response)
            .expect(1)
            .mount(&server)
            .await;
        let error = execute(
            prepare(&["dm", "U1", "--profile", "work", "--yes"], Kind::User).unwrap(),
            &server,
            &credentials,
        )
        .await
        .err()
        .unwrap();
        assert_eq!(error.report().code, expected);
        assert!(
            !serde_json::to_string(&error.report())
                .unwrap()
                .contains("xoxp-test")
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
    }
}

#[tokio::test]
async fn directory_queries_and_dm_creation_remain_bound_to_the_selected_profile() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for args in [
        vec!["users", "list"],
        vec!["resolve", "user", "alice"],
        vec!["dm", "U1", "--profile", "work", "--yes"],
    ] {
        let server = MockServer::start().await;
        let mut response = auth_response();
        response["url"] = json!("https://other.slack.com/");
        Mock::given(path("/api/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .expect(1)
            .mount(&server)
            .await;
        let error = execute(prepare(&args, Kind::User).unwrap(), &server, &credentials)
            .await
            .err()
            .unwrap();
        assert_eq!(error.report().code, "identity_mismatch");
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
    let server = MockServer::start().await;
    identity(&server).await;
    Mock::given(path("/api/users.lookupByEmail"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok": false, "error": "missing_scope"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let error = execute(
        prepare(&["users", "lookup", "alice@example.test"], Kind::User).unwrap(),
        &server,
        &credentials,
    )
    .await
    .err()
    .unwrap();
    assert_eq!(error.report().slack_error, Some("missing_scope"));
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}
