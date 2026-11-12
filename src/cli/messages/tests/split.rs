use super::prepare;
use crate::auth::{Credentials, Kind};
use crate::testing::{auth_response, client};
use serde_json::{Value, json};
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn split_preview_preserves_unicode_and_whitespace_without_writes() {
    let server = MockServer::start().await;
    Mock::given(path("/api/auth.test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(auth_response()))
        .expect(1)
        .mount(&server)
        .await;
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    let text = "  Hello 🦀!\n\n日本語 and a second sentence.\n".repeat(3);
    let result = prepare(
        &[
            "send",
            "C1",
            "--text",
            &text,
            "--split",
            "--chunk-chars",
            "48",
            "--dry-run",
        ],
        Kind::User,
    )
    .unwrap()
    .execute(&client(&server), &credentials)
    .await
    .unwrap();
    let value = serde_json::to_value(result).unwrap();
    let drafts = value["drafts"].as_array().unwrap();
    assert!(drafts.len() > 1);
    assert_eq!(
        drafts
            .iter()
            .map(|draft| draft["text"].as_str().unwrap())
            .collect::<String>(),
        text
    );
    assert!(drafts.iter().all(
        |draft| draft["text"].as_str().unwrap().chars().count() <= 48
            && draft["channel_id"] == "C1"
            && draft["thread_ts"].is_null()
    ));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);

    for args in [
        vec![
            "send",
            "C1",
            "--text",
            "ABCDE",
            "--split",
            "--chunk-chars",
            "2",
            "--max-chunks",
            "2",
            "--dry-run",
        ],
        vec!["send", "C1", "--text", "ABCDE", "--split", "--json"],
    ] {
        assert!(prepare(&args, Kind::User).is_err());
    }
    assert!(
        prepare(
            &["send", "C1", "--text", &"a".repeat(4001), "--dry-run"],
            Kind::User
        )
        .is_err()
    );
}

#[tokio::test]
async fn split_sends_are_ordered_and_stop_with_confirmed_receipts_on_failure() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for reply in [false, true] {
        for failure in [None, Some("slack_error"), Some("write_outcome_unknown")] {
            let server = MockServer::start().await;
            Mock::given(path("/api/auth.test"))
                .respond_with(ResponseTemplate::new(200).set_body_json(auth_response()))
                .mount(&server)
                .await;
            Mock::given(path("/api/conversations.history"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "ok": true, "messages": [{"ts": "1700000000.000001"}]
                })))
                .mount(&server)
                .await;
            Mock::given(path("/api/chat.postMessage"))
                .respond_with(move |request: &wiremock::Request| {
                    let body = request.body_json::<Value>().unwrap();
                    let text = body["text"].as_str().unwrap();
                    if text == "BBBB" {
                        match failure {
                            Some("slack_error") => return ResponseTemplate::new(200).set_body_json(json!({"ok": false, "error": "not_in_channel"})),
                            Some("write_outcome_unknown") => return ResponseTemplate::new(503).set_body_string("unavailable"),
                            _ => (),
                        }
                    }
                    let ts = match text {
                        "AAAA" => "1700000000.000010", "BBBB" => "1700000000.000011", "CCCC" => "1700000000.000012",
                        _ => panic!("unexpected split text"),
                    };
                    ResponseTemplate::new(200).set_body_json(json!({
                        "ok": true, "channel": "C1", "ts": ts, "message": {"thread_ts": body["thread_ts"]}
                    }))
                })
                .expect(if failure.is_some() { 2 } else { 3 })
                .mount(&server).await;
            let args = [
                if reply { "reply" } else { "send" },
                if reply {
                    "https://team.slack.com/archives/C1/p1700000000000001"
                } else {
                    "C1"
                },
                "--text",
                "AAAABBBBCCCC",
                "--split",
                "--chunk-chars",
                "4",
                "--profile",
                "work",
                "--yes",
            ];
            let result = prepare(&args, Kind::User)
                .unwrap()
                .execute(&client(&server), &credentials)
                .await;
            if let Some(code) = failure {
                let error = match result {
                    Ok(_) => panic!("the second chunk must fail"),
                    Err(error) => error,
                };
                let report = serde_json::to_value(error.report()).unwrap();
                assert_eq!(report["code"], "split_write_failed");
                assert_eq!(report["batch"]["failed_chunk"], 2);
                assert_eq!(report["batch"]["total_chunks"], 3);
                assert_eq!(report["batch"]["failure"]["code"], code);
                assert_eq!(report["batch"]["sent"].as_array().unwrap().len(), 1);
                assert_eq!(report["batch"]["sent"][0]["ts"], "1700000000.000010");
            } else {
                let value = serde_json::to_value(result.unwrap()).unwrap();
                assert_eq!(value["data"].as_array().unwrap().len(), 3);
                assert_eq!(value["data"][2]["ts"], "1700000000.000012");
            }
            let writes = server
                .received_requests()
                .await
                .unwrap()
                .into_iter()
                .filter(|request| request.url.path() == "/api/chat.postMessage")
                .map(|request| request.body_json::<Value>().unwrap())
                .collect::<Vec<_>>();
            assert_eq!(
                writes
                    .iter()
                    .map(|body| body["text"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                if failure.is_some() {
                    vec!["AAAA", "BBBB"]
                } else {
                    vec!["AAAA", "BBBB", "CCCC"]
                }
            );
            assert!(writes.iter().all(|body| body["channel"] == "C1"
                && body["thread_ts"]
                    == if reply {
                        json!("1700000000.000001")
                    } else {
                        Value::Null
                    }));
        }
    }
}
