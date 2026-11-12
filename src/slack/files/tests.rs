use super::{FileUrl, Transfer, Upload};
use crate::auth::{Credentials, FileSettings, Kind};
use crate::testing::client;
use bytes::Bytes;
use rustls::pki_types::PrivatePkcs8KeyDer;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::net::SocketAddr;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::{JoinHandle, JoinSet};
use tokio_rustls::TlsAcceptor;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct SecureTransfer {
    transfer: Transfer,
    server: JoinHandle<()>,
}

impl SecureTransfer {
    async fn new(backend: SocketAddr, settings: &FileSettings) -> Self {
        crate::initialize_tls();
        let certificate = rcgen::generate_simple_self_signed(vec![
            "files.slack.com".to_owned(),
            "files.slack-gov.com".to_owned(),
        ])
        .unwrap();
        let tls = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![certificate.cert.der().clone()],
                PrivatePkcs8KeyDer::from(certificate.signing_key.serialize_der()).into(),
            )
            .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(tls));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    socket = listener.accept() => {
                        let (socket, _) = socket.unwrap();
                        let acceptor = acceptor.clone();
                        connections.spawn(async move {
                            let mut stream = acceptor.accept(socket).await.unwrap();
                            let mut upstream = TcpStream::connect(backend).await.unwrap();
                            match tokio::io::copy_bidirectional(&mut stream, &mut upstream).await {
                                Ok(_) => (),
                                Err(error) if matches!(error.kind(), std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::UnexpectedEof) => (),
                                Err(error) => panic!("test TLS relay failed: {error}"),
                            }
                        });
                    }
                    result = connections.join_next(), if !connections.is_empty() => result.unwrap().unwrap(),
                }
            }
        });
        let client = Transfer::client(settings)
            .no_proxy()
            .resolve("files.slack.com", address)
            .resolve("files.slack-gov.com", address)
            .tls_certs_only([reqwest::Certificate::from_der(certificate.cert.der()).unwrap()])
            .build()
            .unwrap();
        Self {
            transfer: Transfer {
                client,
                limit: settings.max_bytes.get(),
            },
            server,
        }
    }
}

impl Drop for SecureTransfer {
    fn drop(&mut self) {
        self.server.abort();
    }
}

#[tokio::test]
async fn uploads_use_three_explicit_stages_and_never_send_credentials_to_the_upload_url() {
    for outcome in ["success", "bad_upload", "ambiguous_complete", "wrong_file"] {
        let api = MockServer::start().await;
        let bytes = MockServer::start().await;
        let secure = SecureTransfer::new(*bytes.address(), &FileSettings::default()).await;
        Mock::given(method("POST")).and(path("/api/files.getUploadURLExternal"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true, "file_id": "F123", "upload_url": "https://files.slack.com/upload/v1/ticket"
            }))).expect(1).mount(&api).await;
        Mock::given(method("POST"))
            .and(path("/upload/v1/ticket"))
            .respond_with(
                ResponseTemplate::new(if outcome == "bad_upload" { 400 } else { 200 })
                    .set_body_string("OK - 3"),
            )
            .expect(1)
            .mount(&bytes)
            .await;
        let complete = if outcome == "ambiguous_complete" {
            ResponseTemplate::new(503)
        } else {
            ResponseTemplate::new(200).set_body_json(json!({
                "ok": true, "files": [{"id": if outcome == "wrong_file" { "F999" } else { "F123" }, "title": "a+é.bin"}]
            }))
        };
        Mock::given(method("POST"))
            .and(path("/api/files.completeUploadExternal"))
            .respond_with(complete)
            .expect(u64::from(outcome != "bad_upload"))
            .mount(&api)
            .await;
        let credentials = Credentials::parse(Kind::Browser, r#"{"token":"xoxc-test","cookie":"xoxd-test%2B","workspace_url":"https://team.slack.com"}"#).unwrap();
        let result = client(&api)
            .upload_file(
                &credentials,
                Upload {
                    name: "a+é.bin".to_owned(),
                    content: Bytes::from_static(&[0, 11, 255]),
                },
                Some(&"C77".into()),
                &secure.transfer,
            )
            .await;
        if outcome == "success" {
            assert_eq!(result.unwrap().file_id.0, "F123");
        } else {
            assert_eq!(
                result.unwrap_err().report().code,
                if outcome == "bad_upload" {
                    "http_error"
                } else {
                    "write_outcome_unknown"
                }
            );
        }
        let uploaded = bytes.received_requests().await.unwrap();
        assert_eq!(uploaded.len(), 1);
        assert_eq!(uploaded[0].body, [0, 11, 255]);
        assert!(!uploaded[0].headers.contains_key("authorization"));
        assert!(!uploaded[0].headers.contains_key("cookie"));
        let requests = api.received_requests().await.unwrap();
        assert_eq!(requests.len(), if outcome == "bad_upload" { 1 } else { 2 });
        assert_eq!(
            requests[0].headers["content-type"],
            "application/x-www-form-urlencoded"
        );
        let form: HashMap<_, _> = url::form_urlencoded::parse(&requests[0].body)
            .into_owned()
            .collect();
        assert_eq!(form["filename"], "a+é.bin");
        assert_eq!(form["length"], "3");
        if requests.len() == 2 {
            assert_eq!(
                requests[1].body_json::<Value>().unwrap(),
                json!({"files": [{"id": "F123", "title": "a+é.bin"}], "channel_id": "C77"})
            );
        }
    }
}

#[tokio::test]
async fn downloads_preserve_bytes_permissions_and_existing_destinations() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/files-pri/synthetic"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes([0, 19, 255, 3]))
        .expect(if cfg!(unix) { 3 } else { 2 })
        .mount(&server)
        .await;
    let settings = FileSettings {
        max_bytes: 4.try_into().unwrap(),
        ..FileSettings::default()
    };
    let secure = SecureTransfer::new(*server.address(), &settings).await;
    let home = tempfile::tempdir().unwrap();
    let output = home.path().join("chosen.bin");
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for exists in [false, true] {
        let url = FileUrl::try_from(
            "https://files.slack.com/files-pri/synthetic"
                .parse::<url::Url>()
                .unwrap(),
        )
        .unwrap();
        let result = secure
            .transfer
            .download(url, &credentials, 4, &output)
            .await;
        if exists {
            assert_eq!(result.unwrap_err().report().code, "io_error");
        } else {
            assert_eq!(result.unwrap(), 4);
        }
        assert_eq!(fs::read(&output).unwrap(), [0, 19, 255, 3]);
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&output).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(fs::read_dir(home.path()).unwrap().count(), 1);
    }
    #[cfg(unix)]
    {
        let link = home.path().join("link.bin");
        std::os::unix::fs::symlink(&output, &link).unwrap();
        let url = FileUrl::try_from(
            "https://files.slack.com/files-pri/synthetic"
                .parse::<url::Url>()
                .unwrap(),
        )
        .unwrap();
        assert!(
            secure
                .transfer
                .download(url, &credentials, 4, &link)
                .await
                .is_err()
        );
        assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(fs::read(&output).unwrap(), [0, 19, 255, 3]);
        assert_eq!(fs::read_dir(home.path()).unwrap().count(), 2);
    }
    let requests = server.received_requests().await.unwrap();
    assert!(
        requests
            .iter()
            .all(|request| request.headers["authorization"] == "Bearer xoxp-test")
    );
    assert!(
        requests
            .iter()
            .all(|request| !request.headers.contains_key("cookie"))
    );
}

#[tokio::test]
async fn gov_downloads_use_the_bound_host_and_reject_cross_realm_credentials() {
    let server = MockServer::start().await;
    Mock::given(path("/original"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"GOV!"))
        .expect(1)
        .mount(&server)
        .await;
    let secure = SecureTransfer::new(*server.address(), &FileSettings::default()).await;
    let home = tempfile::tempdir().unwrap();
    let credentials =
        Credentials::parse(Kind::Bot, r#"{"token":"xoxb-test","realm":"gov"}"#).unwrap();
    let gov = FileUrl::try_from(
        "https://files.slack-gov.com/original"
            .parse::<url::Url>()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        secure
            .transfer
            .download(gov, &credentials, 4, &home.path().join("gov.bin"))
            .await
            .unwrap(),
        4
    );
    assert_eq!(fs::read(home.path().join("gov.bin")).unwrap(), b"GOV!");
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[0].headers["host"], "files.slack-gov.com");
    assert_eq!(requests[0].headers["authorization"], "Bearer xoxb-test");
    assert!(!requests[0].headers.contains_key("cookie"));
    for (credentials, host) in [
        (credentials, "files.slack.com"),
        (
            Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap(),
            "files.slack-gov.com",
        ),
    ] {
        let url = FileUrl::try_from(
            format!("https://{host}/original")
                .parse::<url::Url>()
                .unwrap(),
        )
        .unwrap();
        let error = secure
            .transfer
            .download(url, &credentials, 4, &home.path().join("rejected.bin"))
            .await
            .unwrap_err();
        assert_eq!(error.report().code, "unsafe_file_url");
        assert!(!home.path().join("rejected.bin").exists());
    }
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn redirects_rate_limits_and_size_mismatches_do_not_publish_a_download_or_retry() {
    let other = MockServer::start().await;
    for (response, expected_size, expected) in [
        (
            ResponseTemplate::new(302).insert_header("Location", other.uri()),
            3,
            "http_error",
        ),
        (
            ResponseTemplate::new(429).insert_header("Retry-After", "17"),
            3,
            "rate_limited",
        ),
        (
            ResponseTemplate::new(200).set_body_bytes([1, 2, 3, 4]),
            3,
            "invalid_response",
        ),
        (
            ResponseTemplate::new(200).set_body_bytes([1, 2]),
            3,
            "invalid_response",
        ),
        (ResponseTemplate::new(200), 5, "file_too_large"),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(response)
            .expect(u64::from(expected_size <= 4))
            .mount(&server)
            .await;
        let settings = FileSettings {
            max_bytes: 4.try_into().unwrap(),
            ..FileSettings::default()
        };
        let secure = SecureTransfer::new(*server.address(), &settings).await;
        let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
        let home = tempfile::tempdir().unwrap();
        let url = FileUrl::try_from("https://files.slack.com/bytes".parse::<url::Url>().unwrap())
            .unwrap();
        let error = secure
            .transfer
            .download(
                url,
                &credentials,
                expected_size,
                &home.path().join("result"),
            )
            .await
            .unwrap_err();
        assert_eq!(error.report().code, expected);
        if expected == "rate_limited" {
            assert_eq!(error.report().retry_after, Some(17));
        }
        assert_eq!(fs::read_dir(home.path()).unwrap().count(), 0);
    }
    assert!(other.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn streaming_downloads_enforce_actual_size_and_cleanup_on_timeout_or_cancellation() {
    use axum::{Router, body::Body, routing::get};
    use futures_util::stream;
    use std::convert::Infallible;
    use std::time::Duration;

    let app = Router::new()
        .route(
            "/chunks",
            get(|| async {
                Body::from_stream(stream::iter([
                    Ok::<_, Infallible>(Bytes::from_static(b"ab")),
                    Ok(Bytes::from_static(b"cde")),
                ]))
            }),
        )
        .route(
            "/pending",
            get(|| async {
                Body::from_stream(stream::unfold(true, |first| async move {
                    if !first {
                        std::future::pending::<()>().await;
                    }
                    Some((Ok::<_, Infallible>(Bytes::from_static(b"a")), false))
                }))
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let backend = tokio::spawn(async { axum::serve(listener, app).await.unwrap() });
    for (endpoint, expected_size, limit, expected_error) in [
        ("chunks", 5, 5, None),
        ("chunks", 3, 5, Some("invalid_response")),
        ("chunks", 6, 8, Some("invalid_response")),
        ("chunks", 3, 4, Some("file_too_large")),
        ("pending", 5, 5, Some("request_timeout")),
        ("pending", 5, 5, Some("cancelled")),
    ] {
        let settings = FileSettings {
            max_bytes: limit.try_into().unwrap(),
            timeout_secs: 1.try_into().unwrap(),
        };
        let secure = SecureTransfer::new(address, &settings).await;
        let credentials = Credentials::parse(Kind::Browser, r#"{"token":"xoxc-test","cookie":"xoxd-test%2B","workspace_url":"https://team.slack.com"}"#).unwrap();
        let home = tempfile::tempdir().unwrap();
        let output = home.path().join("result");
        let url = FileUrl::try_from(
            format!("https://files.slack.com/{endpoint}")
                .parse::<url::Url>()
                .unwrap(),
        )
        .unwrap();
        let result = tokio::time::timeout(
            if expected_error == Some("cancelled") {
                Duration::from_millis(100)
            } else {
                Duration::from_secs(5)
            },
            secure
                .transfer
                .download(url, &credentials, expected_size, &output),
        )
        .await;
        match expected_error {
            None => {
                assert_eq!(result.unwrap().unwrap(), 5);
                assert_eq!(fs::read(&output).unwrap(), b"abcde");
            }
            Some("cancelled") => {
                assert!(result.is_err());
                assert_eq!(fs::read_dir(home.path()).unwrap().count(), 0);
            }
            Some(error) => {
                assert_eq!(result.unwrap().unwrap_err().report().code, error);
                assert_eq!(fs::read_dir(home.path()).unwrap().count(), 0);
            }
        }
    }
    backend.abort();
}
