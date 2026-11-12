use super::Slack;
use crate::auth::{Credentials, Kind, Settings};
use crate::error::Error;
use crate::testing::{auth_response, client};
use serde_json::{Value, json};
use slack_morphism::SlackClientHttpConnector;
use std::time::Duration;
use wiremock::matchers::{any, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod conversations;
mod messages;
mod oauth;
mod search;
mod write;

#[tokio::test]
async fn production_connectors_only_target_the_official_or_bound_workspace_api() {
    let user = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    let browser = Credentials::parse(
        Kind::Browser,
        r#"{"token":"xoxc-test","cookie":"xoxd-test","workspace_url":"https://team.slack.com"}"#,
    )
    .unwrap();
    for (credentials, expected) in [
        (user, "https://slack.com/api/auth.test"),
        (browser, "https://team.slack.com/api/auth.test"),
        (
            Credentials::parse(Kind::Bot, r#"{"token":"xoxb-test"}"#).unwrap(),
            "https://slack.com/api/auth.test",
        ),
        (
            Credentials::parse(Kind::User, r#"{"token":"xoxp-test","realm":"gov"}"#).unwrap(),
            "https://slack-gov.com/api/auth.test",
        ),
        (
            Credentials::parse(Kind::Bot, r#"{"token":"xoxb-test","realm":"gov"}"#).unwrap(),
            "https://slack-gov.com/api/auth.test",
        ),
        (
            Credentials::parse(Kind::Browser, r#"{"token":"xoxc-test","cookie":"xoxd-test","workspace_url":"https://agency.slack-gov.com"}"#).unwrap(),
            "https://agency.slack-gov.com/api/auth.test",
        ),
    ] {
        let slack = Slack::connect(
            credentials.realm(),
            credentials.workspace(),
            &Settings::default(),
        )
        .unwrap();
        assert_eq!(
            slack
                .client
                .http_api
                .connector
                .create_method_uri_path("auth.test")
                .unwrap()
                .as_str(),
            expected
        );
    }
}

#[tokio::test]
async fn sdk_sends_bearer_and_optional_cookie_without_credentials_in_url_or_body() {
    for (kind, input, token, cookie) in [
        (Kind::User, json!({"token": "xoxp-test"}), "xoxp-test", None),
        (Kind::Bot, json!({"token": "xoxb-test"}), "xoxb-test", None),
        (
            Kind::Browser,
            json!({"token": "xoxc-test", "cookie": "xoxd-test%2B%3D", "workspace_url": "https://team.slack.com"}),
            "xoxc-test",
            Some("d=xoxd-test%2B%3D"),
        ),
    ] {
        let server = MockServer::start().await;
        let mut response = auth_response();
        if kind == Kind::Bot {
            response["bot_id"] = json!("B7");
        }
        Mock::given(method("POST"))
            .and(path("/api/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .expect(1)
            .mount(&server)
            .await;
        let credentials = Credentials::parse(kind, &input.to_string()).unwrap();
        let identity = client(&server).identify(&credentials).await.unwrap();
        assert_eq!(identity.team_id, "T12345678");
        assert_eq!(identity.user_id, "W87654321");
        assert_eq!(
            identity.bot_id.as_deref(),
            (kind == Kind::Bot).then_some("B7")
        );
        assert_eq!(
            identity.workspace_url.to_string(),
            "https://team.slack.com/"
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(
            request.headers["authorization"].to_str().unwrap(),
            format!("Bearer {token}")
        );
        assert_eq!(
            request
                .headers
                .get("cookie")
                .map(|header| header.to_str().unwrap()),
            cookie
        );
        assert_eq!(request.url.query(), None);
        assert_eq!(request.body_json::<Value>().unwrap(), json!({}));
    }
}

#[tokio::test]
async fn gov_and_bot_authentication_cannot_change_realms_or_identity_kinds() {
    for kind in [Kind::User, Kind::Bot] {
        let credentials = Credentials::parse(
            kind,
            &json!({
                "token": if kind == Kind::Bot { "xoxe.xoxb-test" } else { "xoxp-test" },
                "realm": "gov"
            })
            .to_string(),
        )
        .unwrap();
        for gov in [true, false] {
            for bot in [true, false] {
                let server = MockServer::start().await;
                let mut response = auth_response();
                if gov {
                    response["url"] = json!("https://agency.slack-gov.com/");
                }
                if bot {
                    response["bot_id"] = json!("B7");
                }
                Mock::given(path("/api/auth.test"))
                    .respond_with(ResponseTemplate::new(200).set_body_json(response))
                    .expect(1)
                    .mount(&server)
                    .await;
                let result = client(&server).identify(&credentials).await;
                if bot != (kind == Kind::Bot) {
                    assert_eq!(result.unwrap_err().report().code, "unsupported_identity");
                } else if !gov {
                    assert_eq!(result.unwrap_err().report().code, "identity_mismatch");
                } else {
                    let identity = result.unwrap();
                    assert_eq!(
                        identity.workspace_url.to_string(),
                        "https://agency.slack-gov.com/"
                    );
                    assert_eq!(identity.bot_id.as_deref(), bot.then_some("B7"));
                }
                assert_eq!(server.received_requests().await.unwrap().len(), 1);
            }
        }
    }
}

#[tokio::test]
async fn invalid_success_bots_and_workspace_mismatches_are_rejected() {
    let mut false_ok = auth_response();
    false_ok["ok"] = json!(false);
    let mut bot = auth_response();
    bot["bot_id"] = json!("B12345678");
    let mut wrong_workspace = auth_response();
    wrong_workspace["url"] = json!("https://other.slack.com/");
    let mut untrusted_workspace = auth_response();
    untrusted_workspace["url"] = json!("https://slack.com.evil.example/");
    let mut invalid_identity = auth_response();
    invalid_identity["user_id"] = json!("");
    let credentials = Credentials::parse(
        Kind::Browser,
        r#"{"token":"xoxc-test","cookie":"xoxd-test","workspace_url":"https://team.slack.com"}"#,
    )
    .unwrap();
    for (body, expected) in [
        (false_ok, "invalid_response"),
        (bot, "unsupported_identity"),
        (wrong_workspace, "identity_mismatch"),
        (untrusted_workspace, "invalid_response"),
        (invalid_identity, "invalid_response"),
        (json!({"ok": true}), "invalid_response"),
        (json!({"ok": false}), "invalid_response"),
    ] {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            client(&server)
                .identify(&credentials)
                .await
                .unwrap_err()
                .report()
                .code,
            expected
        );
    }
}

#[tokio::test]
async fn sdk_errors_do_not_leak_response_bodies_or_retry_requests() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for (response, expected, slack_error) in [
        (
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok": false, "error": "missing_scope", "echo": "xoxp-test"})),
            "slack_error",
            Some("missing_scope"),
        ),
        (
            ResponseTemplate::new(200).set_body_json(json!({"ok": false, "error": "xoxp-test"})),
            "slack_error",
            None,
        ),
        (
            ResponseTemplate::new(200).set_body_raw("xoxp-test", "application/json"),
            "invalid_response",
            None,
        ),
        (
            ResponseTemplate::new(503).set_body_string("xoxp-test"),
            "http_error",
            None,
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(response)
            .expect(1)
            .mount(&server)
            .await;
        let error = client(&server).identify(&credentials).await.unwrap_err();
        let report = error.report();
        assert_eq!(report.code, expected);
        assert_eq!(report.slack_error, slack_error);
        assert!(
            !serde_json::to_string(&report)
                .unwrap()
                .contains("xoxp-test")
        );
        assert!(!format!("{error:?} {error}").contains("xoxp-test"));
    }
}

#[tokio::test]
#[ignore = "SDK 429 parsing defect; awaiting a release with https://github.com/abdolence/slack-morphism-rust/pull/397"]
async fn rate_limits_preserve_retry_after_and_make_only_one_request() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for response in [
        ResponseTemplate::new(429).set_body_json(json!({"ok": false, "error": "ratelimited"})),
        ResponseTemplate::new(429).set_body_raw("xoxp-test", "text/plain"),
        ResponseTemplate::new(429),
        ResponseTemplate::new(429).set_body_raw("{invalid", "application/json"),
    ] {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(response.insert_header("Retry-After", "17"))
            .expect(1)
            .mount(&server)
            .await;
        let error = client(&server).identify(&credentials).await.unwrap_err();
        assert_eq!(error.report().code, "rate_limited");
        assert_eq!(error.report().retry_after, Some(17));
    }
}

#[tokio::test]
async fn redirects_never_receive_credentials_and_timeouts_do_not_retry() {
    let server = MockServer::start().await;
    let destination = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_json(auth_response()))
        .expect(0)
        .mount(&destination)
        .await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(307).insert_header("Location", destination.uri()))
        .expect(1)
        .mount(&server)
        .await;
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    assert!(matches!(
        client(&server).identify(&credentials).await,
        Err(Error::Http { status: 307 })
    ));

    let delayed = MockServer::start().await;
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(auth_response())
                .set_delay(Duration::from_secs(1)),
        )
        .expect(1)
        .mount(&delayed)
        .await;
    let mut slack = client(&delayed);
    slack.timeout = Duration::from_millis(200);
    assert!(matches!(
        slack.identify(&credentials).await,
        Err(Error::Timeout)
    ));
}
