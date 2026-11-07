use super::{Login, Settings};
use crate::auth::Workspace;
use crate::error::Error;
use oauth2::{PkceCodeChallenge, PkceCodeVerifier};
use secrecy::ExposeSecret;
use std::collections::HashMap;
use std::future::pending;
use std::net::{Ipv4Addr, TcpListener};
use std::time::Duration;
use tokio::sync::oneshot;
use url::Url;

fn login() -> Login {
    crate::initialize_tls();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    Login {
        client_id: "1234.5678".to_owned(),
        redirect_uri: Url::parse(&format!("http://localhost:{port}/oauth/callback")).unwrap(),
        scopes: vec!["chat:write".to_owned(), "users:read.email".to_owned()],
        workspace: Workspace::try_from("https://team.slack.com".to_owned()).unwrap(),
    }
}

#[tokio::test]
async fn callback_binds_pkce_state_path_and_user_scopes_without_echoing_secrets() {
    let login = login();
    let redirect = login.redirect_uri.clone();
    let settings = Settings::default();
    let (show, shown) = oneshot::channel();
    let authorize = login.authorize(&settings, pending(), |url| {
        show.send(url.clone()).unwrap();
        Ok(())
    });
    let browser = async {
        let url = shown.await.unwrap();
        let params: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(url.origin().ascii_serialization(), "https://slack.com");
        assert_eq!(url.path(), "/oauth/v2/authorize");
        assert_eq!(params["client_id"], "1234.5678");
        assert_eq!(params["redirect_uri"], redirect.as_str());
        assert_eq!(params["response_type"], "code");
        assert_eq!(params["user_scope"], "chat:write,users:read.email");
        assert_eq!(params["code_challenge_method"], "S256");
        for absent in ["client_secret", "code_verifier", "scope"] {
            assert!(!params.contains_key(absent));
        }
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let mut callback = redirect.clone();
        callback
            .query_pairs_mut()
            .append_pair("code", "synthetic-code+/=");
        let missing = client.get(callback.clone()).send().await.unwrap();
        assert_eq!(missing.status(), 403);
        callback
            .query_pairs_mut()
            .append_pair("state", "unrelated-state");
        assert_eq!(
            client.get(callback.clone()).send().await.unwrap().status(),
            403
        );
        callback.set_query(None);
        callback
            .query_pairs_mut()
            .append_pair("state", &params["state"])
            .append_pair("code", "synthetic-code+/=");
        let mut duplicate = callback.clone();
        duplicate
            .query_pairs_mut()
            .append_pair("code", "second-code");
        assert_eq!(client.get(duplicate).send().await.unwrap().status(), 400);
        let mut unrelated = callback.clone();
        unrelated.set_path("/unrelated");
        assert_eq!(client.get(unrelated).send().await.unwrap().status(), 404);
        assert_eq!(
            client.post(callback.clone()).send().await.unwrap().status(),
            404
        );
        let response = client.get(callback).send().await.unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(response.headers()["referrer-policy"], "no-referrer");
        let body = response.text().await.unwrap();
        assert!(!body.contains("synthetic-code"));
        assert!(!body.contains(&params["state"]));
        params
    };
    let (authorization, params) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(authorize, browser)
    })
    .await
    .unwrap();
    let authorization = authorization.unwrap();
    assert_eq!(authorization.code.expose_secret(), "synthetic-code+/=");
    assert_eq!(
        params["code_challenge"],
        PkceCodeChallenge::from_code_verifier_sha256(&PkceCodeVerifier::new(
            authorization.verifier.expose_secret().to_owned()
        ))
        .as_str()
    );
    TcpListener::bind((Ipv4Addr::LOCALHOST, redirect.port().unwrap())).unwrap();
}

#[tokio::test]
async fn denial_cancellation_timeout_and_browser_failure_release_the_callback_port() {
    for outcome in ["denial", "cancel", "timeout", "browser"] {
        let login = login();
        let redirect = login.redirect_uri.clone();
        let settings = Settings {
            login_timeout_secs: 1.try_into().unwrap(),
            ..Settings::default()
        };
        let (show, shown) = oneshot::channel();
        let (cancel, cancelled) = oneshot::channel();
        let authorize = login.authorize(&settings, async { cancelled.await.unwrap() }, |url| {
            show.send(url.clone()).unwrap();
            if outcome == "browser" {
                Err(Error::OAuthBrowser)
            } else {
                Ok(())
            }
        });
        let browser = async {
            let url = shown.await.unwrap();
            if outcome == "denial" {
                let state = url
                    .query_pairs()
                    .find(|(name, _)| name == "state")
                    .unwrap()
                    .1
                    .into_owned();
                let mut callback = redirect.clone();
                callback
                    .query_pairs_mut()
                    .append_pair("state", &state)
                    .append_pair("error", "access_denied");
                assert_eq!(reqwest::get(callback).await.unwrap().status(), 200);
            }
            if outcome == "cancel" {
                cancel.send(()).unwrap();
            } else {
                tokio::time::sleep(Duration::from_millis(1200)).await;
                drop(cancel);
            }
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(4), async {
            tokio::join!(authorize, browser)
        })
        .await
        .unwrap();
        let expected = match outcome {
            "denial" => "oauth_denied",
            "cancel" => "oauth_cancelled",
            "timeout" => "oauth_timeout",
            "browser" => "oauth_browser_failed",
            _ => unreachable!(),
        };
        assert_eq!(result.err().unwrap().report().code, expected);
        TcpListener::bind((Ipv4Addr::LOCALHOST, redirect.port().unwrap())).unwrap();
    }
}

#[tokio::test]
async fn invalid_application_or_callback_is_rejected_before_opening_a_browser() {
    for redirect in [
        "https://localhost:12345/oauth/callback",
        "http://localhost/oauth/callback",
        "http://localhost:0/oauth/callback",
        "http://localhost.evil.example:12345/oauth/callback",
        "http://127.0.0.1:12345/oauth/callback",
        "http://user:secret@localhost:12345/oauth/callback",
        "http://localhost:12345/oauth/callback?state=override",
        "http://localhost:12345/oauth/callback#fragment",
    ] {
        let mut login = login();
        login.redirect_uri = Url::parse(redirect).unwrap();
        let error = login
            .authorize(&Settings::default(), pending(), |_| {
                panic!("unexpected browser launch")
            })
            .await
            .err()
            .unwrap();
        assert_eq!(error.report().code, "invalid_arguments");
    }
    for (client_id, scopes) in [
        ("not-an-id", vec!["chat:write"]),
        ("1234.5678", vec![]),
        ("1234.5678", vec!["chat:write users:read"]),
    ] {
        let mut login = login();
        login.client_id = client_id.to_owned();
        login.scopes = scopes.into_iter().map(str::to_owned).collect();
        let error = login
            .authorize(&Settings::default(), pending(), |_| {
                panic!("unexpected browser launch")
            })
            .await
            .err()
            .unwrap();
        assert_eq!(error.report().code, "invalid_arguments");
    }
}
