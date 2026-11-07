use super::{Session, capture};
use crate::auth::{Settings, Workspace};
use crate::slack::tests::{auth_response, client};
use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use chromiumoxide::Page;
use chromiumoxide::cdp::browser_protocol::fetch::{
    EnableParams, EventRequestPaused, FulfillRequestParams, HeaderEntry,
};
use chromiumoxide::cdp::browser_protocol::network::CookieParam;
use chromiumoxide::cdp::browser_protocol::page::EventLoadEventFired;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::future::pending;
use std::net::TcpStream;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;
use tokio::time::{Instant, timeout};
use url::Url;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn fixture(page: &Page, mode: &str) -> tokio::task::JoinHandle<()> {
    let mut requests = page.event_listener::<EventRequestPaused>().await.unwrap();
    page.execute(EnableParams::default()).await.unwrap();
    let page = page.clone();
    let html =
        include_str!("tests/login.html").replace("__MODE__", &serde_json::to_string(mode).unwrap());
    tokio::spawn(async move {
        while let Some(request) = requests.next().await {
            let document = request.request.url == "https://team.slack.com/";
            let body = if document { html.as_bytes() } else { b"{}" };
            let response = FulfillRequestParams::builder()
                .request_id(request.request_id.clone())
                .response_code(200)
                .response_headers([
                    HeaderEntry::new(
                        "Content-Type",
                        if document {
                            "text/html"
                        } else {
                            "application/json"
                        },
                    ),
                    HeaderEntry::new("Access-Control-Allow-Origin", "*"),
                    HeaderEntry::new(
                        "Set-Cookie",
                        "d=xoxd-selected%2Bplus%2F%3D; Secure; HttpOnly; Path=/",
                    ),
                ])
                .body(BASE64_STANDARD.encode(body))
                .build()
                .unwrap();
            if page.execute(response).await.is_err() {
                break;
            }
        }
    })
}

#[tokio::test]
#[ignore = "Requires Chrome/Chromium and a desktop or Xvfb; all pages and credentials are synthetic"]
async fn native_browser_capture_binds_workspace_and_preserves_httponly_cookie() {
    for mode in ["json", "form", "multipart", "malformed"] {
        let session = Session::launch(
            &Settings::default(),
            Instant::now() + Duration::from_secs(10),
        )
        .await
        .unwrap();
        let directory = session.directory.path().to_owned();
        #[cfg(unix)]
        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let debugger = Url::parse(session.browser.websocket_address()).unwrap();
        let address = (debugger.host_str().unwrap(), debugger.port().unwrap());
        let page = session.browser.new_page("about:blank").await.unwrap();
        page.set_cookie(
            CookieParam::builder()
                .name("d")
                .value("xoxd-other")
                .url("https://unrelated.example/")
                .http_only(true)
                .secure(true)
                .build()
                .unwrap(),
        )
        .await
        .unwrap();
        let fixture = fixture(&page, mode).await;
        let workspace = Workspace::try_from("https://team.slack.com/".to_owned()).unwrap();
        let captured = timeout(Duration::from_secs(10), capture(&page, workspace)).await;
        let document_cookie = page
            .evaluate("document.cookie")
            .await
            .unwrap()
            .into_value::<String>()
            .unwrap();
        session.close().await.unwrap();
        fixture.abort();
        if let Err(error) = fixture.await {
            assert!(error.is_cancelled());
        }
        assert!(
            !directory.exists(),
            "temporary browser data survived {mode}"
        );
        assert!(
            TcpStream::connect(address).is_err(),
            "debug port survived {mode}"
        );
        assert_eq!(document_cookie, "");

        let captured = captured.expect("the fixture must produce a session or a parse error");
        if mode == "malformed" {
            let error = captured.unwrap_err();
            assert_eq!(error.report().code, "browser_capture_failed");
            assert!(
                !serde_json::to_string(&error.report())
                    .unwrap()
                    .contains("xox")
            );
            continue;
        }
        let credentials = captured.unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&credentials.encode_for_keyring().unwrap()).unwrap(),
            json!({
                "token": "xoxc-selected_123",
                "cookie": "xoxd-selected%2Bplus%2F%3D",
                "workspace_url": "https://team.slack.com/",
            })
        );
        let server = MockServer::start().await;
        for valid in [true, false] {
            server.reset().await;
            let mut response = auth_response();
            if !valid {
                response["url"] = json!("https://other.slack.com/");
            }
            Mock::given(method("POST"))
                .and(path("/api/auth.test"))
                .and(header("authorization", "Bearer xoxc-selected_123"))
                .and(header("cookie", "d=xoxd-selected%2Bplus%2F%3D"))
                .respond_with(ResponseTemplate::new(200).set_body_json(response))
                .expect(1)
                .mount(&server)
                .await;
            let identity = client(&server).identify(&credentials).await;
            if valid {
                assert_eq!(identity.unwrap().user_id, "W87654321");
            } else {
                assert_eq!(identity.unwrap_err().report().code, "identity_mismatch");
            }
        }
    }
}

#[tokio::test]
#[ignore = "Requires Chrome/Chromium and a desktop or Xvfb; no Slack requests are made"]
async fn native_browser_cancellation_timeout_and_closed_page_release_resources() {
    for cancel in [true, false] {
        let session = Session::launch(
            &Settings::default(),
            Instant::now() + Duration::from_secs(10),
        )
        .await
        .unwrap();
        let directory = session.directory.path().to_owned();
        let debugger = Url::parse(session.browser.websocket_address()).unwrap();
        let address = (debugger.host_str().unwrap(), debugger.port().unwrap());
        let workspace = Workspace::try_from("https://team.slack.com/".to_owned()).unwrap();
        let error = session
            .acquire(workspace, Instant::now(), async {
                if !cancel {
                    pending::<()>().await;
                }
            })
            .await
            .unwrap_err();
        assert_eq!(
            error.report().code,
            if cancel {
                "browser_login_cancelled"
            } else {
                "browser_login_timeout"
            }
        );
        assert!(!directory.exists());
        assert!(TcpStream::connect(address).is_err());
    }

    let session = Session::launch(
        &Settings::default(),
        Instant::now() + Duration::from_secs(10),
    )
    .await
    .unwrap();
    let directory = session.directory.path().to_owned();
    let debugger = Url::parse(session.browser.websocket_address()).unwrap();
    let address = (debugger.host_str().unwrap(), debugger.port().unwrap());
    let page = session.browser.new_page("about:blank").await.unwrap();
    let fixture = fixture(&page, "idle").await;
    let mut loaded = page.event_listener::<EventLoadEventFired>().await.unwrap();
    let workspace = Workspace::try_from("https://team.slack.com/".to_owned()).unwrap();
    let close = async {
        loaded.next().await.unwrap();
        page.clone().close().await.unwrap();
    };
    let captured = timeout(Duration::from_secs(10), async {
        tokio::join!(capture(&page, workspace), close).0
    })
    .await;
    session.close().await.unwrap();
    fixture.abort();
    if let Err(error) = fixture.await {
        assert!(error.is_cancelled());
    }
    assert!(!directory.exists());
    assert!(TcpStream::connect(address).is_err());
    assert_eq!(
        captured.unwrap().unwrap_err().report().code,
        "browser_closed"
    );
}
