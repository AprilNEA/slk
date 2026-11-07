use super::client;
use crate::auth::{Credentials, Kind};
use crate::messages::MessageLink;
use crate::slack::{CursorPage, ThreadRequest};
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn channels_preserve_types_and_a_short_page_with_a_cursor() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/conversations.list"))
        .and(query_param("types", "public_channel,private_channel,mpim,im"))
        .and(query_param("limit", "7"))
        .and(query_param("cursor", "first+/="))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "channels": [
                {"id": "C1", "name": "general", "is_channel": true},
                {"id": "C2", "name": "private", "is_channel": true, "is_private": true, "is_archived": true},
                {"id": "G3", "is_mpim": true, "is_private": true},
                {"id": "D4", "user": "U9", "is_im": true, "is_private": true}
            ],
            "response_metadata": {"next_cursor": "next+/="}
        })))
        .expect(1).mount(&server).await;
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    let page = client(&server)
        .channels(
            &credentials,
            &CursorPage {
                limit: 7,
                cursor: Some("first+/=".into()),
                max_pages: 1,
            },
        )
        .await
        .unwrap();
    let output = serde_json::to_value(page).unwrap();
    let types: Vec<_> = output["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["conversation_type"].as_str().unwrap())
        .collect();
    assert_eq!(types, ["public_channel", "private_channel", "mpim", "im"]);
    assert_eq!(output["data"][1]["is_archived"], true);
    assert_eq!(output["data"][3]["user_id"], "U9");
    assert_eq!(output["pagination"], json!({"next_cursor": "next+/="}));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn history_and_replies_keep_api_order_and_ignore_unknown_message_shapes() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    let channel = "C1".into();
    let root = "1700000000.000001".into();
    let page = CursorPage {
        limit: 7,
        cursor: Some("opaque+/=".into()),
        max_pages: 1,
    };
    for is_thread in [false, true] {
        let server = MockServer::start().await;
        let mut messages = vec![
            json!({"ts": "1700000000.000001", "user": "U1", "text": "root"}),
            json!({"ts": "1700000000.000002", "thread_ts": "1700000000.000001", "bot_id": "B2", "text": "\u{001b}]52;c;unsafe\u{0007}reply", "subtype": "future_subtype", "blocks": [{"type": "future_block"}], "extra": {"ignored": true}}),
        ];
        if !is_thread {
            messages.extend([
                json!({"ts": "1700000000.000003", "user": "U3", "text": "joined", "subtype": "channel_join"}),
                json!({"ts": "1700000000.000004", "user": "U4", "text": "left", "subtype": "channel_leave"}),
            ]);
            messages.reverse();
        }
        Mock::given(method("GET"))
            .and(path(if is_thread { "/api/conversations.replies" } else { "/api/conversations.history" }))
            .and(query_param("channel", "C1"))
            .and(query_param("limit", "7"))
            .and(query_param("cursor", "opaque+/="))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true, "messages": messages, "has_more": true, "response_metadata": {"next_cursor": "later"}})))
            .expect(1).mount(&server).await;
        let slack = client(&server);
        let result = if is_thread {
            slack
                .replies(
                    &credentials,
                    &ThreadRequest {
                        channel_id: &channel,
                        thread_ts: &root,
                        page: &page,
                    },
                )
                .await
        } else {
            slack.history(&credentials, &channel, &page).await
        }
        .unwrap();
        let output = serde_json::to_value(result).unwrap();
        let times: Vec<_> = output["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["ts"].as_str().unwrap())
            .collect();
        assert_eq!(
            times,
            if is_thread {
                vec!["1700000000.000001", "1700000000.000002"]
            } else {
                vec![
                    "1700000000.000004",
                    "1700000000.000003",
                    "1700000000.000002",
                    "1700000000.000001",
                ]
            }
        );
        let reply = output["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["ts"] == "1700000000.000002")
            .unwrap();
        assert_eq!(reply["author"], "B2");
        assert_eq!(reply["channel_id"], "C1");
        assert_eq!(reply["thread_ts"], "1700000000.000001");
        assert_eq!(reply["text"], "\u{001b}]52;c;unsafe\u{0007}reply");
        assert_eq!(
            output["pagination"],
            json!({"next_cursor": "later", "has_more": true})
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        if is_thread {
            assert!(
                requests[0]
                    .url
                    .query_pairs()
                    .any(|(key, value)| key == "ts" && value == "1700000000.000001")
            );
        }
    }
}

#[tokio::test]
async fn automatic_pagination_crosses_empty_pages_and_preserves_budget_continuations() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for (budget, expected, next, requests) in [
        (1, vec!["C1"], Some("second"), 1),
        (2, vec!["C1"], Some("third"), 2),
        (5, vec!["C1", "C3"], None, 3),
    ] {
        let server = MockServer::start().await;
        Mock::given(path("/api/conversations.list"))
            .respond_with(|request: &wiremock::Request| {
                let cursor = request.url.query_pairs().find(|(key, _)| key == "cursor");
                let (channels, next) = match cursor.as_ref().map(|(_, value)| value.as_ref()) {
                    None => (json!([{"id": "C1", "name": "one"}]), "second"),
                    Some("second") => (json!([]), "third"),
                    Some("third") => (json!([{"id": "C3", "name": "three"}]), ""),
                    _ => panic!("unexpected continuation"),
                };
                ResponseTemplate::new(200).set_body_json(json!({
                    "ok": true, "channels": channels, "response_metadata": {"next_cursor": next}
                }))
            })
            .expect(requests)
            .mount(&server)
            .await;
        let page = client(&server)
            .channels(
                &credentials,
                &CursorPage {
                    limit: 7,
                    cursor: None,
                    max_pages: budget,
                },
            )
            .await
            .unwrap();
        let value = serde_json::to_value(page).unwrap();
        assert_eq!(
            value["data"]
                .as_array()
                .unwrap()
                .iter()
                .map(|channel| channel["channel_id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(value["pagination"]["next_cursor"], json!(next));
    }
}

#[tokio::test]
async fn automatic_pagination_rejects_cycles_and_does_not_return_partial_data_on_failure() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for (second, expected) in [
        (
            json!({"ok": true, "channels": [], "response_metadata": {"next_cursor": "again"}}),
            "invalid_response",
        ),
        (
            json!({"ok": false, "error": "invalid_cursor"}),
            "slack_error",
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(path("/api/conversations.list"))
            .respond_with(move |request: &wiremock::Request| {
                let response = if request.url.query_pairs().any(|(key, _)| key == "cursor") {
                    second.clone()
                } else {
                    json!({"ok": true, "channels": [{"id": "C1"}], "response_metadata": {"next_cursor": "again"}})
                };
                ResponseTemplate::new(200).set_body_json(response)
            })
            .expect(2)
            .mount(&server).await;
        let error = client(&server)
            .channels(
                &credentials,
                &CursorPage {
                    limit: 7,
                    cursor: None,
                    max_pages: 5,
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error.report().code, expected);
    }
}

#[tokio::test]
async fn thread_roots_are_confirmed_and_reply_links_are_checked_for_membership() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for (link, explicit, broadcast, subtype) in [
        (
            "https://team.slack.com/archives/C1/p1700000000000001",
            None,
            false,
            None,
        ),
        (
            "https://team.slack.com/archives/C1/p1700000000000002?thread_ts=1700000000.000001",
            None,
            false,
            Some("future_subtype"),
        ),
        (
            "https://team.slack.com/archives/C1/p1700000000000002",
            Some("1700000000.000001".into()),
            false,
            Some("bot_message"),
        ),
        (
            "https://team.slack.com/archives/C1/p1700000000000002",
            None,
            true,
            Some("future_subtype"),
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(path("/api/conversations.history"))
            .and(query_param("latest", "1700000000.000001"))
            .and(query_param("oldest", "1700000000.000001"))
            .and(query_param("inclusive", "true"))
            .and(query_param("limit", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true, "messages": [{"ts": "1700000000.000001", "thread_ts": "1700000000.000001", "subtype": subtype}]})))
            .expect(1).mount(&server).await;
        let reply = json!({"ts": "1700000000.000002", "thread_ts": "1700000000.000001", "subtype": "thread_broadcast"});
        if broadcast {
            Mock::given(path("/api/conversations.history"))
                .and(query_param("latest", "1700000000.000002"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(json!({"ok": true, "messages": [reply]})),
                )
                .expect(1)
                .mount(&server)
                .await;
        } else if !link.ends_with("p1700000000000001") {
            Mock::given(path("/api/conversations.replies"))
                .and(query_param("ts", "1700000000.000001"))
                .and(query_param("oldest", "1700000000.000002"))
                .and(query_param("latest", "1700000000.000002"))
                .and(query_param("inclusive", "true"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(json!({"ok": true, "messages": [reply]})),
                )
                .expect(1)
                .mount(&server)
                .await;
        }
        let link: MessageLink = link.parse().unwrap();
        let root = client(&server)
            .resolve_thread(&credentials, &link, explicit.as_ref())
            .await
            .unwrap();
        assert_eq!(root.0, "1700000000.000001");
        assert!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .all(|request| request.method.as_str() == "GET")
        );
    }
}

#[tokio::test]
async fn missing_or_unrelated_messages_never_become_guessed_roots() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for messages in [json!([]), json!([{"ts": "1699999999.999999"}])] {
        let server = MockServer::start().await;
        Mock::given(path("/api/conversations.history"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"ok": true, "messages": messages})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let link = "https://team.slack.com/archives/C1/p1700000000000002"
            .parse()
            .unwrap();
        let error = client(&server)
            .resolve_thread(&credentials, &link, None)
            .await
            .unwrap_err();
        assert_eq!(error.report().code, "thread_unresolved");
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
    let server = MockServer::start().await;
    Mock::given(path("/api/conversations.history"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok": true, "messages": [{"ts": "1700000000.000001"}]})),
        )
        .mount(&server)
        .await;
    Mock::given(path("/api/conversations.replies"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true, "messages": [{"ts": "1700000000.000002", "thread_ts": "1699999999.999999"}]})))
        .mount(&server).await;
    let link = "https://team.slack.com/archives/C1/p1700000000000002?thread_ts=1700000000.000001"
        .parse()
        .unwrap();
    assert_eq!(
        client(&server)
            .resolve_thread(&credentials, &link, None)
            .await
            .unwrap_err()
            .report()
            .code,
        "thread_unresolved"
    );
}

#[tokio::test]
async fn empty_pages_are_distinct_from_permissions_and_expired_cursor_errors() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for error in [
        None,
        Some("missing_scope"),
        Some("channel_not_found"),
        Some("invalid_cursor"),
    ] {
        let server = MockServer::start().await;
        let body = error.map_or_else(
            || json!({"ok": true, "messages": [], "response_metadata": {"next_cursor": ""}}),
            |error| json!({"ok": false, "error": error}),
        );
        Mock::given(path("/api/conversations.history"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;
        let result = client(&server)
            .history(
                &credentials,
                &"C1".into(),
                &CursorPage {
                    limit: 7,
                    cursor: None,
                    max_pages: 1,
                },
            )
            .await;
        if let Some(expected) = error {
            assert_eq!(result.unwrap_err().report().slack_error, Some(expected));
        } else {
            let output: Value = serde_json::to_value(result.unwrap()).unwrap();
            assert_eq!(
                output,
                json!({"data": [], "pagination": {"next_cursor": null}})
            );
        }
    }
}
