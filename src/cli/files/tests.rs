use super::Prepared;
use crate::auth::{Credentials, Kind};
use crate::cli::{Cli, Command};
use crate::error::Result;
use crate::testing::{self, auth_response, client};
use clap::Parser;
use serde_json::{Value, json};
use std::fs;
use wiremock::matchers::{path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn prepare(args: &[&str]) -> Result<Prepared> {
    let (_directory, store) = testing::store(&["work"], Kind::User, json!({}));
    let cli = Cli::try_parse_from(std::iter::once("slk").chain(args.iter().copied())).unwrap();
    let Command::Files { command, selector } = cli.command else {
        panic!("expected files command")
    };
    let selected = selector.resolve(&store, cli.profile.as_deref())?;
    command.prepare(&store, selected.as_deref(), cli.json)
}

#[tokio::test]
async fn upload_preview_is_private_by_default_and_does_not_request_an_upload_ticket() {
    let source = tempfile::NamedTempFile::new().unwrap();
    fs::write(source.path(), b"example").unwrap();
    for channel in [None, Some("C9")] {
        let server = MockServer::start().await;
        Mock::given(path("/api/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(auth_response()))
            .expect(1)
            .mount(&server)
            .await;
        let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
        let mut args = vec![
            "files",
            "upload",
            source.path().to_str().unwrap(),
            "--dry-run",
            "--json",
        ];
        if let Some(channel) = channel {
            args.extend(["--channel", channel]);
        }
        let result = prepare(&args)
            .unwrap()
            .execute(&client(&server), &credentials)
            .await
            .unwrap();
        let result = serde_json::to_value(result).unwrap();
        assert_eq!(result["dry_run"], true);
        assert_eq!(result["profile"], "work");
        assert_eq!(result["size"], 7);
        assert_eq!(result["channel_id"], json!(channel));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
    for flags in [vec![], vec!["--yes"], vec!["--profile", "work"]] {
        let mut args = vec!["files", "upload", source.path().to_str().unwrap(), "--json"];
        args.extend(flags);
        assert_eq!(
            prepare(&args).err().unwrap().report().code,
            "invalid_arguments"
        );
    }
}

#[tokio::test]
async fn file_metadata_hides_private_urls_preserves_text_and_rejects_wrong_ids() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for id in ["F1", "F9"] {
        let server = MockServer::start().await;
        Mock::given(path("/api/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(auth_response()))
            .mount(&server)
            .await;
        Mock::given(path("/api/files.info")).and(query_param("file", "F1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true, "file": {
                "id": id, "title": "visible\u{001b}]52;discarded", "size": 7,
                "url_private": "https://files.slack.com/private", "url_private_download": "https://files.slack.com/download"
            }}))).expect(1).mount(&server).await;
        let result = prepare(&["files", "info", "F1"])
            .unwrap()
            .execute(&client(&server), &credentials)
            .await;
        if id == "F9" {
            assert_eq!(result.err().unwrap().report().code, "invalid_response");
        } else {
            let result = result.unwrap();
            let mut text = Vec::new();
            result.write(false, &mut text).unwrap();
            assert_eq!(String::from_utf8(text).unwrap(), "F1\tvisible\t7 bytes\n");
            let json: Value = serde_json::to_value(result).unwrap();
            assert_eq!(json["title"], "visible\u{001b}]52;discarded");
            assert!(json.get("url_private").is_none());
            assert!(json.get("url_private_download").is_none());
        }
    }
}

#[tokio::test]
async fn file_commands_stop_before_data_access_when_identity_changes() {
    let source = tempfile::NamedTempFile::new().unwrap();
    fs::write(source.path(), b"example").unwrap();
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for args in [
        vec!["files", "info", "F1"],
        vec!["files", "download", "F1", "--output", "result.bin"],
        vec![
            "files",
            "upload",
            source.path().to_str().unwrap(),
            "--profile",
            "work",
            "--yes",
        ],
    ] {
        let server = MockServer::start().await;
        let mut response = auth_response();
        response["user_id"] = json!("U99");
        Mock::given(path("/api/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .expect(1)
            .mount(&server)
            .await;
        let error = prepare(&args)
            .unwrap()
            .execute(&client(&server), &credentials)
            .await
            .err()
            .unwrap();
        assert_eq!(error.report().code, "identity_mismatch");
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
    assert_eq!(
        prepare(&["files", "download", "F1", "--output", "-"])
            .err()
            .unwrap()
            .report()
            .code,
        "invalid_arguments"
    );
}
