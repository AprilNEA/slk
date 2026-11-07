use super::{Plan, Updater};
use base64::{Engine, engine::general_purpose::STANDARD};
use self_update::{Release, ReleaseAsset};
use std::fs;
use std::time::Duration;
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

const TARGET: &str = "x86_64-unknown-linux-gnu";
const ARCHIVE: &str = "slk-x86_64-unknown-linux-gnu.tar.xz";
// Deterministic tar.xz and Deflate ZIP fixtures contain "new executable\n" in cargo-dist's layout.
const TAR: &str = "/Td6WFoAAATm1rRGAgAhARYAAAB0L+Wj4Cf/AIJdADmbCaUYyVaBTCbwdiwgPtXpKiZQwRvTAfuav8t+AlrDAjptEz94xc0FMBzVz/FJfByTX6LAMKtpTG1EQv3KLeS/DPVH73iI0DbvaC3XmbHsHz7/PhdYotDgNd7C27Dl6nYFZEdlFeTgPgX9fKA3R0cfodf3qWHaaMYDfMXcLz4bu6AAAABTdTlno56qLAABngGAUAAA1RK8WLHEZ/sCAAAAAARZWg==";
const TAR_DIGEST: &str = "sha256:cac00653181dc7f2db61f584d8079f3560ee6da7b29fbc8b2c8b87b3c1b70e05";
const ZIP: &str = "UEsDBBQAAAAIAAAAIQAiF9eBEQAAAA8AAAAHAAAAc2xrLmV4ZctLLVdIrUhNLi1JTMpJ5QIAUEsBAhQDFAAAAAgAAAAhACIX14ERAAAADwAAAAcAAAAAAAAAAAAAAIABAAAAAHNsay5leGVQSwUGAAAAAAEAAQA1AAAANgAAAAAA";
const ZIP_DIGEST: &str = "sha256:24efa025f778181af1b79efc69d30c368f470cef211296873d25cff60c7f3a06";
const WRONG_TAR: &str = "/Td6WFoAAATm1rRGAgAhARYAAAB0L+Wj4Cf/AGddADedCTPSYstVudPhLL91kkeZTVHP4Zb0oL1dj4vIQK5BvX5jf8NRhPzWdAdAlm8zhZOIhLPTtnypaoOQFoDuKQAWlpjrPvNgGhP+/Y9hFc96tJARsrr5TovBZGNtheRfdLTk6p2ZXwAAAIYIOPdBIG8fAAGDAYBQAACQA/TnscRn+wIAAAAABFla";
const WRONG_TAR_DIGEST: &str =
    "sha256:f0b80c8bd810e5149c88280d8bf044fa0a3ba07a87241b4c064ec346d6af3373";
const WRONG_ZIP: &str = "UEsDBBQAAAAIAAAAIQAiF9eBEQAAAA8AAAAJAAAAb3RoZXIuZXhly0stV0itSE0uLUlMyknlAgBQSwECFAMUAAAACAAAACEAIhfXgREAAAAPAAAACQAAAAAAAAAAAAAAgAEAAAAAb3RoZXIuZXhlUEsFBgAAAAABAAEANwAAADgAAAAAAA==";
const WRONG_ZIP_DIGEST: &str =
    "sha256:0da261e3e06f6d8bf09d91350c88973d9620b68cd5d8d71eadac28667d49d8c4";

fn release(version: &str, assets: Vec<ReleaseAsset>) -> Release {
    Release::builder()
        .version(version)
        .assets(assets)
        .build()
        .unwrap()
}

#[test]
fn release_policy_requires_a_new_stable_version_and_one_exact_sha256_asset() {
    let asset = ReleaseAsset::new(ARCHIVE, "https://example.test/release").with_digest(TAR_DIGEST);
    for (version, available) in [
        ("0.9.9", false),
        ("1.0.0", false),
        ("1.0.0+different-build", false),
        ("1.0.1", true),
        ("2.0.0", true),
    ] {
        let plan = Plan::new(release(version, vec![asset.clone()]), "1.0.0", TARGET).unwrap();
        assert_eq!(plan.available(), available);
    }
    assert!(Plan::new(release("2.0.0-rc.1", vec![asset.clone()]), "1.0.0", TARGET).is_err());
    for assets in [
        vec![],
        vec![ReleaseAsset::new(
            format!("{ARCHIVE}.sha256"),
            "https://example.test/sums",
        )],
        vec![
            ReleaseAsset::new(
                "slk-x86_64-unknown-linux-musl.tar.xz",
                "https://example.test/musl",
            )
            .with_digest(TAR_DIGEST),
        ],
        vec![asset.clone(), asset.clone()],
        vec![ReleaseAsset::new(ARCHIVE, "https://example.test/no-digest")],
    ] {
        assert!(Plan::new(release("2.0.0", assets), "1.0.0", TARGET).is_err());
    }
    for digest in [
        "md5:abc",
        "sha512:abc",
        "sha256:abc",
        &format!("sha256:{}", "z".repeat(64)),
    ] {
        assert!(
            Plan::new(
                release(
                    "2.0.0",
                    vec![
                        ReleaseAsset::new(ARCHIVE, "https://example.test/invalid")
                            .with_digest(digest)
                    ]
                ),
                "1.0.0",
                TARGET
            )
            .is_err()
        );
    }
    let plan = Plan::new(
        release(
            "2.0.0",
            vec![
                ReleaseAsset::new(format!("{ARCHIVE}.sha256"), "https://example.test/sums"),
                asset,
            ],
        ),
        "1.0.0",
        TARGET,
    )
    .unwrap();
    assert_eq!(plan.asset.unwrap().name(), ARCHIVE);
}

#[tokio::test]
async fn verified_archive_installation_and_failures_preserve_the_declared_result() {
    crate::initialize_tls();
    for (target, archive, correct, missing) in [
        (
            TARGET,
            ARCHIVE,
            (TAR, TAR_DIGEST),
            (WRONG_TAR, WRONG_TAR_DIGEST),
        ),
        (
            "x86_64-pc-windows-msvc",
            "slk-x86_64-pc-windows-msvc.zip",
            (ZIP, ZIP_DIGEST),
            (WRONG_ZIP, WRONG_ZIP_DIGEST),
        ),
    ] {
        for failure in [None, Some("digest"), Some("member"), Some("http")] {
            let server = MockServer::start().await;
            let (bytes, digest) = if failure == Some("member") {
                missing
            } else {
                correct
            };
            let response = if failure == Some("http") {
                ResponseTemplate::new(503)
            } else {
                ResponseTemplate::new(200).set_body_bytes(STANDARD.decode(bytes).unwrap())
            };
            Mock::given(path("/asset"))
                .respond_with(response)
                .expect(1)
                .mount(&server)
                .await;
            let asset = ReleaseAsset::new(archive, format!("{}/asset", server.uri())).with_digest(
                if failure == Some("digest") {
                    format!("sha256:{}", "0".repeat(64))
                } else {
                    digest.to_owned()
                },
            );
            let plan = Plan::new(release("2.0.0", vec![asset]), "1.0.0", target).unwrap();
            let temporary = tempfile::tempdir().unwrap();
            let destination = temporary.path().join("slk-test");
            fs::write(&destination, b"old executable").unwrap();
            let updater = Updater {
                client: reqwest::Client::new(),
                timeout: Duration::from_secs(2),
            };
            let result = updater.install(plan, &destination).await;
            if failure.is_some() {
                assert!(result.is_err(), "{failure:?}");
                assert_eq!(fs::read(&destination).unwrap(), b"old executable");
            } else {
                let report = serde_json::to_value(result.unwrap()).unwrap();
                assert_eq!(report["state"], "updated");
                assert_eq!(report["latest_version"], "2.0.0");
                assert_eq!(fs::read(&destination).unwrap(), b"new executable\n");
            }
            let requests = server.received_requests().await.unwrap();
            assert_eq!(requests.len(), 1);
            assert!(!requests[0].headers.contains_key("authorization"));
            assert!(!requests[0].headers.contains_key("cookie"));
        }
    }
}

#[tokio::test]
async fn production_update_transport_rejects_plain_http_without_sending_a_request() {
    let server = MockServer::start().await;
    let updater = Updater::new(&crate::auth::Settings::default()).unwrap();
    assert!(updater.client.get(server.uri()).send().await.is_err());
    assert!(server.received_requests().await.unwrap().is_empty());
}
