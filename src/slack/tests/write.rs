use crate::auth::{Credentials, Kind};
use crate::model::conversations::ConversationAction;
use crate::model::messages::{Change, Draft, Message, MessageText};
use crate::slack::Slack;
use crate::testing::client;
use hyper_util::client::legacy::connect::HttpConnector;
use serde_json::{Value, json};
use slack_morphism::api::{
    SlackApiCanvasesEditRequest, SlackCanvasChange, SlackCanvasDocumentContent,
    SlackCanvasOperation,
};
use slack_morphism::hyper_tokio::SlackClientHyperConnector;
use std::io::{BufRead, BufReader, Read};
use std::net::TcpListener;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn draft() -> Draft {
    Draft {
        channel_id: "C1".into(),
        thread_ts: None,
        content: MessageText::try_from("literal $(do-not-execute)\n🦀".to_owned())
            .unwrap()
            .into(),
    }
}

#[tokio::test]
async fn writes_post_once_with_no_broadcast_unfurls_or_followup_requests() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for thread_ts in [None, Some("1700000000.000001".into())] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/chat.postMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true, "channel": "C1", "ts": "1700000000.000009",
                "message": {"thread_ts": thread_ts, "subtype": "future_subtype", "blocks": [{"type": "future"}]}
            })))
            .expect(1).mount(&server).await;
        let mut draft = draft();
        draft.thread_ts = thread_ts.clone();
        let result = client(&server).send(&credentials, &draft).await.unwrap();
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            json!({"channel_id": "C1", "ts": "1700000000.000009", "thread_ts": thread_ts})
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let mut expected = json!({
            "channel": "C1", "text": "literal $(do-not-execute)\n🦀",
            "reply_broadcast": false, "unfurl_links": false, "unfurl_media": false,
            "link_names": false, "parse": "none"
        });
        if let Some(root) = thread_ts {
            expected["thread_ts"] = json!(root);
        }
        assert_eq!(requests[0].body_json::<Value>().unwrap(), expected);
    }
}

#[tokio::test]
async fn ambiguous_writes_are_not_reported_as_rejections_or_retried() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for response in [
        ResponseTemplate::new(503).set_body_string("xoxp-test"),
        ResponseTemplate::new(200).set_body_string("invalid xoxp-test JSON"),
        ResponseTemplate::new(200).set_body_json(json!({"ok": true, "channel": "C1"})),
        ResponseTemplate::new(200).set_body_json(json!({"ok": false})),
        ResponseTemplate::new(200).set_body_json(json!({"ok": false, "error": "internal_error"})),
        ResponseTemplate::new(200).set_body_json(json!({"ok": false, "error": "fatal_error"})),
        ResponseTemplate::new(200).set_body_json(
            json!({"ok": true, "channel": "C2", "ts": "1700000000.000009", "message": {}}),
        ),
        ResponseTemplate::new(200)
            .set_body_json(json!({"ok": true, "channel": "C1", "ts": "", "message": {}})),
        ResponseTemplate::new(200).set_delay(Duration::from_secs(1)),
    ] {
        let server = MockServer::start().await;
        Mock::given(path("/api/chat.postMessage"))
            .respond_with(response)
            .expect(1)
            .mount(&server)
            .await;
        let mut slack = client(&server);
        slack.timeout = Duration::from_millis(100);
        let error = slack.send(&credentials, &draft()).await.unwrap_err();
        assert_eq!(error.report().code, "write_outcome_unknown");
        assert!(
            !serde_json::to_string(&error.report())
                .unwrap()
                .contains("xoxp-test")
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn explicit_rejections_and_well_formed_rate_limits_remain_distinct() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for (response, expected_code, expected_slack) in [
        (
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok": false, "error": "missing_scope"})),
            "slack_error",
            "missing_scope",
        ),
        (
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok": false, "error": "channel_not_found"})),
            "slack_error",
            "channel_not_found",
        ),
        (
            ResponseTemplate::new(429)
                .insert_header("Retry-After", "17")
                .set_body_json(json!({"ok": false, "error": "ratelimited"})),
            "rate_limited",
            "ratelimited",
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(path("/api/chat.postMessage"))
            .respond_with(response)
            .expect(1)
            .mount(&server)
            .await;
        let error = client(&server)
            .send(&credentials, &draft())
            .await
            .unwrap_err();
        assert_eq!(error.report().code, expected_code);
        assert_eq!(error.report().slack_error, Some(expected_slack));
        if expected_code == "rate_limited" {
            assert_eq!(error.report().retry_after, Some(17));
        }
    }
}

#[tokio::test]
async fn a_server_that_accepts_the_body_then_disconnects_receives_only_one_write() {
    for method in [
        "chat.postMessage",
        "chat.update",
        "chat.delete",
        "reactions.add",
        "reactions.remove",
        "pins.add",
        "pins.remove",
        "conversations.open",
        "conversations.create",
        "conversations.join",
        "conversations.leave",
        "conversations.archive",
        "conversations.setTopic",
        "conversations.mark",
        "conversations.close",
        "canvases.edit",
        "chat.scheduleMessage",
        "chat.deleteScheduledMessage",
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut reader = BufReader::new(stream);
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            assert_eq!(request_line, format!("POST /api/{method} HTTP/1.1\r\n"));
            let mut line = String::new();
            let mut length = None;
            loop {
                line.clear();
                assert_ne!(reader.read_line(&mut line).unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = Some(value.trim().parse::<usize>().unwrap());
                }
            }
            let mut body = vec![0; length.unwrap()];
            reader.read_exact(&mut body).unwrap();
            drop(reader);
            (listener, serde_json::from_slice::<Value>(&body).unwrap())
        });
        let slack = Slack::new(
            SlackClientHyperConnector::with_connector(HttpConnector::new())
                .with_slack_api_url(&format!("http://{address}/api")),
            Duration::from_secs(2),
        );
        let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
        let error = match method {
            "chat.postMessage" => slack.send(&credentials, &draft()).await.unwrap_err(),
            "conversations.open" => slack.open_dm(&credentials, &"U7".into()).await.unwrap_err(),
            "conversations.create" => slack
                .create_channel(&credentials, "review-room", true)
                .await
                .unwrap_err(),
            method if method.starts_with("conversations.") => {
                let action = match method {
                    "conversations.join" => ConversationAction::Join,
                    "conversations.leave" => ConversationAction::Leave,
                    "conversations.archive" => ConversationAction::Archive,
                    "conversations.setTopic" => ConversationAction::Topic {
                        topic: "New topic".to_owned(),
                    },
                    "conversations.mark" => ConversationAction::Mark {
                        ts: "1700000000.000006".into(),
                    },
                    "conversations.close" => ConversationAction::CloseGroup,
                    _ => unreachable!(),
                };
                slack
                    .change_conversation(&credentials, &"C1".into(), &action)
                    .await
                    .unwrap_err()
            }
            "canvases.edit" => slack
                .edit_canvas(
                    &credentials,
                    &SlackApiCanvasesEditRequest::new(
                        "F7".into(),
                        vec![
                            SlackCanvasChange::new(SlackCanvasOperation::Replace)
                                .with_section_id("temp:C:section".into())
                                .with_document_content(SlackCanvasDocumentContent::new(
                                    "New section".to_owned(),
                                )),
                        ],
                    ),
                )
                .await
                .unwrap_err(),
            "chat.scheduleMessage" => slack
                .schedule(&credentials, &draft(), 1_794_000_000)
                .await
                .unwrap_err(),
            "chat.deleteScheduledMessage" => slack
                .cancel_schedule(
                    &credentials,
                    &crate::model::scheduled::Receipt {
                        channel_id: "C1".into(),
                        scheduled_message_id: "Q24".to_owned(),
                        post_at: 1_794_000_000,
                    },
                )
                .await
                .unwrap_err(),
            _ => {
                let message = Message {
                    channel_id: "C1".into(),
                    ts: "1700000000.000002".into(),
                    thread_ts: Some("1700000000.000001".into()),
                    author: Some("U1".into()),
                    text: "original".into(),
                    blocks: None,
                    permalink: None,
                };
                let change = match method {
                    "chat.update" => Change::Edit {
                        content: draft().content,
                    },
                    "chat.delete" => Change::Delete,
                    "reactions.add" => Change::React {
                        name: "wave".into(),
                    },
                    "reactions.remove" => Change::Unreact {
                        name: "wave".into(),
                    },
                    "pins.add" => Change::Pin,
                    "pins.remove" => Change::Unpin,
                    _ => unreachable!(),
                };
                slack
                    .change(&credentials, message, change)
                    .await
                    .unwrap_err()
            }
        };
        assert_eq!(error.report().code, "write_outcome_unknown", "{method}");
        let (listener, accepted) = server.join().unwrap();
        if method == "conversations.open" {
            assert_eq!(accepted, json!({"users": ["U7"], "return_im": true}));
        } else if method == "conversations.create" {
            assert_eq!(accepted, json!({"name": "review-room", "is_private": true}));
        } else if method.starts_with("conversations.") {
            let mut expected = json!({"channel": "C1"});
            if method == "conversations.setTopic" {
                expected["topic"] = json!("New topic");
            }
            if method == "conversations.mark" {
                expected["ts"] = json!("1700000000.000006");
            }
            assert_eq!(accepted, expected);
        } else if method == "canvases.edit" {
            assert_eq!(
                accepted,
                json!({"canvas_id": "F7", "changes": [{"operation": "replace", "section_id": "temp:C:section", "document_content": {"type": "markdown", "markdown": "New section"}}]})
            );
        } else if method == "chat.scheduleMessage" {
            assert_eq!(accepted["channel"], "C1");
            assert_eq!(accepted["post_at"], 1_794_000_000);
        } else if method == "chat.deleteScheduledMessage" {
            assert_eq!(
                accepted,
                json!({"channel": "C1", "scheduled_message_id": "Q24"})
            );
        } else {
            assert_eq!(accepted["channel"], "C1");
            if method.starts_with("reactions.") {
                assert_eq!(accepted["timestamp"], "1700000000.000002");
                assert_eq!(accepted["name"], "wave");
            } else if method.starts_with("pins.") {
                assert_eq!(
                    accepted,
                    json!({"channel": "C1", "timestamp": "1700000000.000002"})
                );
            } else if method != "chat.postMessage" {
                assert_eq!(accepted["ts"], "1700000000.000002");
            }
            if matches!(method, "chat.postMessage" | "chat.update") {
                assert_eq!(accepted["text"], "literal $(do-not-execute)\n🦀");
            }
        }
        listener.set_nonblocking(true).unwrap();
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}
