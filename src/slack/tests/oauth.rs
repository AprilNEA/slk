use crate::auth::oauth::{Authorization, Login};
use crate::auth::{Credentials, Identity, Kind, Workspace};
use crate::testing::{auth_response, client};
use secrecy::ExposeSecret;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn authorization() -> Authorization {
    Authorization {
        login: Login {
            client_id: "1234.5678".to_owned(),
            redirect_uri: "http://localhost:12345/oauth/callback".parse().unwrap(),
            scopes: vec!["chat:write".to_owned()],
            workspace: Workspace::try_from("https://team.slack.com".to_owned()).unwrap(),
        },
        code: "synthetic-code".into(),
        verifier: "synthetic-verifier".into(),
    }
}

fn token() -> Value {
    json!({
        "access_token": "xoxe.xoxp-new-access", "token_type": "user", "scope": "chat:write",
        "refresh_token": "xoxe-new-refresh", "expires_in": 43200
    })
}

#[tokio::test]
async fn oauth_exchange_requests_only_user_grants_and_binds_the_verified_identity() {
    for rotating in [false, true] {
        let server = MockServer::start().await;
        let mut user = token();
        if !rotating {
            user.as_object_mut().unwrap().remove("refresh_token");
            user.as_object_mut().unwrap().remove("expires_in");
            user["access_token"] = json!("xoxp-nonrotating");
        }
        user["id"] = json!("W87654321");
        Mock::given(method("POST"))
            .and(path("/api/oauth.v2.access"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true, "authed_user": user, "team": {"id": "T12345678"},
                "future_field": true
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(path("/api/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(auth_response()))
            .expect(1)
            .mount(&server)
            .await;
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let mut authorization = authorization();
        authorization.code = "synthetic-code+&= %".into();
        let (credentials, identity) = client(&server)
            .oauth_exchange(&authorization)
            .await
            .unwrap();
        assert_eq!(identity.team_id, "T12345678");
        assert_eq!(identity.user_id, "W87654321");
        assert_eq!(
            identity.workspace_url.to_string(),
            "https://team.slack.com/"
        );
        if rotating {
            let rotation = credentials.rotation().unwrap();
            assert_eq!(rotation.client_id, "1234.5678");
            assert_eq!(rotation.refresh_token.expose_secret(), "xoxe-new-refresh");
            assert_eq!(rotation.scope, "chat:write");
            assert!((before + 43200..=before + 43205).contains(&rotation.expires_at));
        } else {
            assert_eq!(
                credentials.rotation().unwrap_err().report().code,
                "not_rotating"
            );
        }
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2);
        let exchange = &requests[0];
        assert_eq!(
            exchange.headers["content-type"],
            "application/x-www-form-urlencoded"
        );
        let fields: BTreeMap<_, _> = url::form_urlencoded::parse(&exchange.body).collect();
        assert_eq!(
            serde_json::to_value(fields).unwrap(),
            json!({
                "grant_type": "authorization_code", "client_id": "1234.5678",
                "code": "synthetic-code+&= %", "code_verifier": "synthetic-verifier",
                "redirect_uri": "http://localhost:12345/oauth/callback"
            })
        );
        assert!(!exchange.headers.contains_key("authorization"));
        assert!(!exchange.headers.contains_key("cookie"));
        assert_eq!(exchange.url.query(), None);
        assert_eq!(
            requests[1].headers["authorization"],
            format!("Bearer {}", user["access_token"].as_str().unwrap())
        );
    }
}

#[tokio::test]
async fn oauth_rejects_other_workspaces_users_teams_and_inconsistent_rotation() {
    for fault in [
        "workspace",
        "user",
        "team",
        "bot",
        "missing_refresh",
        "missing_expiry",
        "invalid_token",
    ] {
        let server = MockServer::start().await;
        let mut user = token();
        user["id"] = json!("W87654321");
        match fault {
            "bot" => user["token_type"] = json!("bot"),
            "missing_refresh" => {
                user.as_object_mut().unwrap().remove("refresh_token");
            }
            "missing_expiry" => {
                user.as_object_mut().unwrap().remove("expires_in");
            }
            "invalid_token" => user["access_token"] = json!("unexpected-token"),
            _ => (),
        }
        let mut identity = auth_response();
        match fault {
            "workspace" => identity["url"] = json!("https://other.slack.com/"),
            "user" => identity["user_id"] = json!("U22222222"),
            "team" => identity["team_id"] = json!("T33333333"),
            _ => (),
        }
        Mock::given(path("/api/oauth.v2.access"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true, "authed_user": user, "team": {"id": "T12345678"}
            })))
            .expect(1)
            .mount(&server)
            .await;
        let mismatch = matches!(fault, "workspace" | "user" | "team");
        Mock::given(path("/api/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(identity))
            .expect(u64::from(mismatch))
            .mount(&server)
            .await;
        let error = client(&server)
            .oauth_exchange(&authorization())
            .await
            .unwrap_err();
        assert_eq!(
            error.report().code,
            if mismatch {
                "identity_mismatch"
            } else {
                "invalid_response"
            }
        );
        assert!(!error.to_string().contains("xox"));
    }
}

#[tokio::test]
async fn refresh_returns_the_whole_new_pair_or_requires_reauthorization_without_replay() {
    let credentials = Credentials::parse(
        Kind::User,
        &json!({
            "token": "xoxe.xoxp-old-access", "rotation": {
                "client_id": "1234.5678", "refresh_token": "xoxe-old-refresh", "expires_at": 1,
                "scope": "chat:write"
            }
        })
        .to_string(),
    )
    .unwrap();
    let identity = Identity {
        workspace_url: Workspace::try_from("https://team.slack.com".to_owned()).unwrap(),
        team_id: "T12345678".to_owned(),
        user_id: "W87654321".to_owned(),
        bot_id: None,
    };
    for valid in [true, false] {
        let server = MockServer::start().await;
        let mut response = token();
        response["ok"] = json!(true);
        Mock::given(path("/api/oauth.v2.access"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .expect(1)
            .mount(&server)
            .await;
        let mut remote = auth_response();
        if !valid {
            remote["user_id"] = json!("U33333333");
        }
        Mock::given(path("/api/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(remote))
            .expect(1)
            .mount(&server)
            .await;
        let result = client(&server).oauth_refresh(&credentials, &identity).await;
        if valid {
            let refreshed = result.unwrap();
            let stored: Value =
                serde_json::from_str(&refreshed.encode_for_keyring().unwrap()).unwrap();
            assert_eq!(stored["token"], "xoxe.xoxp-new-access");
            assert_eq!(stored["rotation"]["refresh_token"], "xoxe-new-refresh");
            assert!(!stored.to_string().contains("old-"));
        } else {
            assert_eq!(result.unwrap_err().report().code, "oauth_refresh_not_saved");
        }
        assert_eq!(
            credentials
                .rotation()
                .unwrap()
                .refresh_token
                .expose_secret(),
            "xoxe-old-refresh"
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[0].headers["content-type"],
            "application/x-www-form-urlencoded"
        );
        let fields: BTreeMap<_, _> = url::form_urlencoded::parse(&requests[0].body).collect();
        assert_eq!(
            serde_json::to_value(fields).unwrap(),
            json!({
                "grant_type": "refresh_token", "client_id": "1234.5678", "refresh_token": "xoxe-old-refresh"
            })
        );
        assert!(!requests[0].headers.contains_key("authorization"));
        assert!(!requests[0].headers.contains_key("cookie"));
    }
    let server = MockServer::start().await;
    let nonrotating = Credentials::parse(Kind::User, r#"{"token":"xoxp-imported"}"#).unwrap();
    assert_eq!(
        client(&server)
            .oauth_refresh(&nonrotating, &identity)
            .await
            .unwrap_err()
            .report()
            .code,
        "not_rotating"
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn consumed_or_uncertain_oauth_grants_are_never_retried_or_echoed() {
    for (response, expected) in [
        (
            ResponseTemplate::new(503).set_body_string("synthetic-verifier"),
            "oauth_outcome_unknown",
        ),
        (
            ResponseTemplate::new(200).set_body_string("synthetic-code"),
            "oauth_outcome_unknown",
        ),
        (
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok": false, "error": "invalid_grant"})),
            "slack_error",
        ),
        (
            ResponseTemplate::new(200).set_delay(Duration::from_secs(1)),
            "oauth_outcome_unknown",
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(path("/api/oauth.v2.access"))
            .respond_with(response)
            .expect(1)
            .mount(&server)
            .await;
        let mut slack = client(&server);
        slack.timeout = Duration::from_millis(100);
        let error = slack.oauth_exchange(&authorization()).await.unwrap_err();
        assert_eq!(error.report().code, expected);
        let output = format!(
            "{error:?} {error} {}",
            serde_json::to_string(&error.report()).unwrap()
        );
        assert!(!output.contains("synthetic-"));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}
