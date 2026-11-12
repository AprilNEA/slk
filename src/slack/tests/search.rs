use crate::auth::{Credentials, Kind};
use crate::model::messages::SearchPage;
use crate::testing::client;
use serde_json::json;
use wiremock::matchers::{body_partial_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn official_search_projects_rts_fields_and_preserves_cursor_pagination() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/assistant.search.context"))
        .and(body_partial_json(json!({"query": "in:<#C1> deploy", "cursor": "first+/=", "limit": 20, "content_types": ["messages"], "include_context_messages": false})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "results": {"messages": [{"channel_id": "C1", "message_ts": "1700000000.000009", "author_user_id": "U1", "content": "deploy result", "permalink": "https://team.slack.com/archives/C1/p1700000000000009", "is_author_bot": false, "extra": "ignored"}]},
            "response_metadata": {"next_cursor": "second+/="}
        })))
        .expect(1).mount(&server).await;
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    let result = client(&server)
        .search(
            &credentials,
            "in:<#C1> deploy",
            &SearchPage {
                limit: 20,
                cursor: Some("first+/=".into()),
                page: None,
                max_pages: 1,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        json!({
            "data": [{"channel_id": "C1", "ts": "1700000000.000009", "thread_ts": null, "author": "U1", "text": "deploy result", "permalink": "https://team.slack.com/archives/C1/p1700000000000009"}],
            "pagination": {"next_cursor": "second+/="}
        })
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn official_search_errors_never_fall_back_to_legacy_search() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for code in [
        "missing_scope",
        "feature_not_enabled",
        "invalid_auth",
        "invalid_cursor",
    ] {
        let server = MockServer::start().await;
        Mock::given(path("/api/assistant.search.context"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"ok": false, "error": code})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let error = client(&server)
            .search(
                &credentials,
                "deploy",
                &SearchPage {
                    limit: 20,
                    cursor: None,
                    page: None,
                    max_pages: 1,
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error.report().slack_error, Some(code));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn browser_search_uses_page_metadata_instead_of_result_count() {
    let credentials = Credentials::parse(
        Kind::Browser,
        r#"{"token":"xoxc-test","cookie":"xoxd-test","workspace_url":"https://team.slack.com"}"#,
    )
    .unwrap();
    for (requested_page, total_pages, matches, expected_next) in [
        (
            2,
            5,
            json!([{"channel": {"id": "D1"}, "ts": "1700000000.000002", "user": "U1", "text": "result", "subtype": "future"}]),
            Some(3),
        ),
        (5, 5, json!([]), None),
        (100, 101, json!([]), None),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/search.messages"))
            .and(query_param("query", "a&b + deploy"))
            .and(query_param("count", "20"))
            .and(query_param("page", requested_page.to_string()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true, "messages": {"matches": matches, "paging": {"page": requested_page, "pages": total_pages}}})))
            .expect(1).mount(&server).await;
        let result = client(&server)
            .search(
                &credentials,
                "a&b + deploy",
                &SearchPage {
                    limit: 20,
                    cursor: None,
                    page: Some(requested_page),
                    max_pages: 1,
                },
            )
            .await
            .unwrap();
        let value = serde_json::to_value(result).unwrap();
        assert_eq!(value["pagination"]["next_page"], json!(expected_next));
        assert_eq!(
            value["pagination"]["has_more"],
            requested_page < total_pages
        );
        if expected_next.is_some() {
            assert_eq!(value["data"][0]["channel_id"], "D1");
            assert_eq!(value["data"][0]["ts"], "1700000000.000002");
        } else {
            assert_eq!(value["data"], json!([]));
        }
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].headers["cookie"], "d=xoxd-test");
    }
}

#[tokio::test]
async fn invalid_search_pagination_never_contacts_an_api() {
    let server = MockServer::start().await;
    let user = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    let browser = Credentials::parse(
        Kind::Browser,
        r#"{"token":"xoxc-test","cookie":"xoxd-test","workspace_url":"https://team.slack.com"}"#,
    )
    .unwrap();
    for (credentials, page) in [
        (
            &user,
            SearchPage {
                limit: 20,
                cursor: None,
                page: Some(1),
                max_pages: 1,
            },
        ),
        (
            &browser,
            SearchPage {
                limit: 20,
                cursor: Some("wrong".into()),
                page: None,
                max_pages: 1,
            },
        ),
        (
            &user,
            SearchPage {
                limit: 21,
                cursor: None,
                page: None,
                max_pages: 1,
            },
        ),
    ] {
        assert_eq!(
            client(&server)
                .search(credentials, "deploy", &page)
                .await
                .unwrap_err()
                .report()
                .code,
            "invalid_arguments"
        );
    }
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}

#[tokio::test]
async fn automatic_browser_pagination_keeps_order_and_exposes_unread_pages() {
    let credentials = Credentials::parse(
        Kind::Browser,
        r#"{"token":"xoxc-test","cookie":"xoxd-test","workspace_url":"https://team.slack.com"}"#,
    )
    .unwrap();
    for budget in [1, 2, 5] {
        let server = MockServer::start().await;
        Mock::given(path("/api/search.messages"))
            .respond_with(|request: &wiremock::Request| {
                let page = request.url.query_pairs().find(|(key, _)| key == "page")
                    .unwrap().1.parse::<u32>().unwrap();
                ResponseTemplate::new(200).set_body_json(json!({"ok": true, "messages": {
                    "matches": [{"channel": {"id": "C1"}, "ts": format!("1700000000.{page:06}"), "text": format!("page {page}")}],
                    "paging": {"page": page, "pages": 3}
                }}))
            })
            .expect(u64::from(budget.min(3)))
            .mount(&server).await;
        let output = client(&server)
            .search(
                &credentials,
                "release",
                &SearchPage {
                    limit: 7,
                    page: None,
                    cursor: None,
                    max_pages: budget,
                },
            )
            .await
            .unwrap();
        let value = serde_json::to_value(output).unwrap();
        let expected = ["page 1", "page 2", "page 3"];
        assert_eq!(
            value["data"]
                .as_array()
                .unwrap()
                .iter()
                .map(|message| message["text"].as_str().unwrap())
                .collect::<Vec<_>>(),
            &expected[..usize::from(budget.min(3))]
        );
        assert_eq!(value["pagination"]["has_more"], budget < 3);
        assert_eq!(
            value["pagination"]["next_page"],
            if budget < 3 {
                json!(budget + 1)
            } else {
                json!(null)
            }
        );
    }
}
