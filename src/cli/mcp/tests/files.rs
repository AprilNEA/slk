use super::{Cli, Server, http::Http, slack_server};
use crate::cli::{Command, files::FileCommand};
use crate::error::Error;
use crate::output::{Output, files};
use crate::testing::auth_response;
use base64::{Engine, prelude::BASE64_STANDARD};
use serde_json::json;
use std::sync::{Arc, Mutex};
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn mcp_file_inputs_never_resolve_server_paths_or_stdin() {
    let http = Http::new(Server::new(
        "work".into(),
        |_| async { Err(Error::CredentialMissing) },
        true,
    ))
    .await;
    let link = "https://team.slack.com/archives/C1/p1700000000000007";
    for args in [
        vec!["send", "C1", "--file", "body.txt", "--yes"],
        vec!["reply", link, "--file", "body.txt", "--yes"],
        vec!["edit", link, "--file", "body.txt", "--yes"],
        vec![
            "send",
            "C1",
            "--text",
            "body",
            "--blocks-file",
            "body.txt",
            "--yes",
        ],
        vec![
            "edit",
            link,
            "--text",
            "body",
            "--blocks-file",
            "body.txt",
            "--yes",
        ],
        vec![
            "messages",
            "send",
            "--recipient-id",
            "C1",
            "--message-file",
            "body.txt",
            "--yes",
        ],
        vec![
            "messages",
            "edit",
            "--permalink",
            link,
            "--message-file",
            "body.txt",
            "--yes",
        ],
        vec![
            "schedule",
            "create",
            "C1",
            "--at",
            "2030-01-01T00:00:00Z",
            "--file",
            "body.txt",
            "--yes",
        ],
        vec![
            "schedule",
            "reply",
            link,
            "--at",
            "2030-01-01T00:00:00Z",
            "--text",
            "body",
            "--blocks-file",
            "body.txt",
            "--yes",
        ],
        vec!["canvas", "append", "F1", "--file", "body.txt", "--yes"],
        vec!["canvas", "prepend", "F1", "--file", "body.txt", "--yes"],
        vec![
            "canvas",
            "insert-before",
            "F1",
            "temp:C:one",
            "--file",
            "body.txt",
            "--yes",
        ],
        vec![
            "canvas",
            "insert-after",
            "F1",
            "temp:C:one",
            "--file",
            "body.txt",
            "--yes",
        ],
        vec![
            "canvas",
            "replace-section",
            "F1",
            "temp:C:one",
            "--file",
            "body.txt",
            "--yes",
        ],
        vec!["files", "upload", "body.txt", "--yes"],
        vec!["files", "download", "F1", "--output", "body.txt"],
    ] {
        for (name, code) in [
            ("body.txt", "credential_missing"),
            ("../escape.txt", "invalid_arguments"),
        ] {
            let args = args
                .iter()
                .map(|arg| if *arg == "body.txt" { name } else { *arg })
                .collect::<Vec<_>>();
            let result = http
                .call(
                    "slk_write",
                    json!({"args": args, "files": {"body.txt": "Ym9keQ=="}}),
                )
                .await;
            assert_eq!(result.structured_content.unwrap()["code"], code, "{args:?}");
        }
    }
    for name in [
        "",
        "-",
        "/private/session.json",
        "C:\\private\\session.json",
        "C:session.json",
        "\\\\server\\share",
        "NUL",
        "name.",
        "trailing ",
        "line\nbreak",
    ] {
        let result = http
            .call(
                "slk_write",
                json!({
                    "args": ["send", "C1", "--file", name, "--yes"], "files": {(name): "Ym9keQ=="}
                }),
            )
            .await;
        assert_eq!(
            result.structured_content.unwrap()["code"],
            "invalid_arguments",
            "{name:?}"
        );
    }
    for files in [
        json!({}),
        json!({"body.txt": "not base64!"}),
        json!({"body.txt": "Ym9keQ==", "../session": "Ym9keQ=="}),
    ] {
        let result = http
            .call(
                "slk_write",
                json!({"args": ["send", "C1", "--file", "body.txt", "--yes"], "files": files}),
            )
            .await;
        assert_eq!(
            result.structured_content.unwrap()["code"],
            "invalid_arguments"
        );
    }
}

#[tokio::test]
async fn mcp_upload_previews_preserve_filenames_and_obey_the_existing_file_size_limit() {
    let api = MockServer::start().await;
    Mock::given(path("/api/auth.test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(auth_response()))
        .expect(1)
        .mount(&api)
        .await;
    let http = Http::new(slack_server(&api)).await;
    let result = http
        .call(
            "slk_write",
            json!({
                "args": ["files", "upload", "a+é.bin", "--dry-run"],
                "files": {"a+é.bin": BASE64_STANDARD.encode(b"one\0two\n")}
            }),
        )
        .await;
    assert_eq!(result.is_error, Some(false));
    let value = result.structured_content.unwrap();
    assert_eq!(value["dry_run"], true);
    assert_eq!(value["filename"], "a+é.bin");
    assert_eq!(value["size"], 8);
    assert_eq!(value["channel_id"], json!(null));
    let result = http
        .call(
            "slk_write",
            json!({
                "args": ["files", "upload", "large.bin", "--yes"],
                "files": {"large.bin": BASE64_STANDARD.encode(b"ninebytes")}
            }),
        )
        .await;
    assert_eq!(result.is_error, Some(true));
    assert_eq!(result.structured_content.unwrap()["code"], "file_too_large");
    assert_eq!(api.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn mcp_downloads_return_exact_bytes_and_remove_temporary_files_on_success_and_error() {
    let paths = Arc::new(Mutex::new(Vec::new()));
    let downloads = Arc::clone(&paths);
    let http = Http::new(Server::new(
        "work".into(),
        move |cli: Cli| {
            let downloads = Arc::clone(&downloads);
            async move {
                let Command::Files {
                    command: FileCommand::Download { file, output },
                    ..
                } = cli.command
                else {
                    panic!("expected a download");
                };
                assert!(output.is_absolute());
                downloads.lock().unwrap().push(output.clone());
                tokio::fs::write(&output, b"\0\xff\nA\x7f").await.unwrap();
                if file.to_string() == "F2" {
                    return Err(Error::Transport);
                }
                Ok(Output::Files(files::Output::Downloaded {
                    file_id: file,
                    path: output,
                    size: 5,
                }))
            }
        },
        true,
    ))
    .await;
    let result = http
        .call(
            "slk_write",
            json!({"args": ["files", "download", "F1", "--output", "a+é.bin"]}),
        )
        .await;
    assert_eq!(result.is_error, Some(false));
    assert_eq!(
        result.structured_content.unwrap(),
        json!({"file_id": "F1", "path": "a+é.bin", "size": 5, "content_base64": "AP8KQX8="})
    );
    let result = http
        .call(
            "slk_write",
            json!({"args": ["files", "download", "F2", "--output", "a+é.bin"]}),
        )
        .await;
    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        result.structured_content.unwrap()["code"],
        "transport_error"
    );
    let paths = paths.lock().unwrap();
    assert_eq!(paths.len(), 2);
    assert_ne!(paths[0].parent(), paths[1].parent());
    assert!(paths.iter().all(|path| !path.parent().unwrap().exists()));
}
