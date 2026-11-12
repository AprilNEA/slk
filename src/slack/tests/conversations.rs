use crate::auth::{Credentials, Kind};
use crate::model::pagination::CursorPage;
use crate::testing::client;
use serde_json::json;
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
