use super::Prepared;
use crate::app::Connection;
use crate::auth::{Credentials, Kind, ProfileView};
use crate::cli::{Cli, Command, write::Terminal};
use crate::error::{Error, Result};
use crate::output::Output;
use crate::testing::{self, auth_response, client};
use clap::Parser;
use serde_json::json;
use std::fs;
use std::time::Duration;
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn prepare(args: &[&str]) -> Result<(ProfileView, Prepared)> {
    let (_directory, store) = testing::store(&["work"], Kind::User, json!({}));
    let cli = Cli::try_parse_from(["slk", "canvas"].into_iter().chain(args.iter().copied()))
        .map_err(|_| Error::Arguments("Invalid test command."))?;
    let Command::Canvas { command } = cli.command else {
        panic!("expected canvas command")
    };
    command.prepare(&store, cli.profile.as_deref(), cli.json)
}

async fn execute(
    (profile, prepared): (ProfileView, Prepared),
    server: &MockServer,
    credentials: &Credentials,
) -> Result<Output> {
    let connection = Connection::verify(client(server), credentials.clone(), profile).await?;
    prepared.execute(connection, &Terminal).await
}

async fn identity(server: &MockServer) {
    Mock::given(path("/api/auth.test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(auth_response()))
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test]
async fn canvas_reads_preserve_content_and_section_ids_without_editing_or_rendering_html() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for (args, endpoint, payload, response, expected) in [
        (
            vec!["read", "F7"],
            "canvases.getContent",
            json!({"canvas_id": "F7", "content_type": "markdown"}),
            json!({"ok": true, "content": ""}),
            json!({"canvas_id": "F7", "content_type": "markdown", "content": ""}),
        ),
        (
            vec!["read", "F7", "--format", "html"],
            "canvases.getContent",
            json!({"canvas_id": "F7", "content_type": "html"}),
            json!({"ok": true, "content": "<script>neverRun()</script>\u{001b}]52;hidden"}),
            json!({"canvas_id": "F7", "content_type": "html", "content": "<script>neverRun()</script>\u{001b}]52;hidden"}),
        ),
        (
            vec![
                "sections",
                "F7",
                "--contains",
                "Release & owners",
                "--section-type",
                "h1,h2",
            ],
            "canvases.sections.lookup",
            json!({"canvas_id": "F7", "criteria": {"contains_text": "Release & owners", "section_types": ["h1", "h2"]}}),
            json!({"ok": true, "sections": [{"id": "temp:C:second"}, {"id": "temp:C:first"}]}),
            json!({"canvas_id": "F7", "sections": [{"id": "temp:C:second"}, {"id": "temp:C:first"}]}),
        ),
        (
            vec!["sections", "F7", "--contains", "No match"],
            "canvases.sections.lookup",
            json!({"canvas_id": "F7", "criteria": {"contains_text": "No match"}}),
            json!({"ok": true, "sections": []}),
            json!({"canvas_id": "F7", "sections": []}),
        ),
    ] {
        let server = MockServer::start().await;
        identity(&server).await;
        Mock::given(method("POST"))
            .and(path(format!("/api/{endpoint}")))
            .and(body_json(payload))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .expect(1)
            .mount(&server)
            .await;
        let result = execute(prepare(&args).unwrap(), &server, &credentials)
            .await
            .unwrap();
        assert_eq!(serde_json::to_value(&result).unwrap(), expected);
        let mut text = Vec::new();
        result.write(false, &mut text).unwrap();
        let text = String::from_utf8(text).unwrap();
        assert!(!text.contains('\x1b'));
        assert!(!text.contains("hidden"));
        if args.contains(&"html") {
            assert!(text.contains("<script>neverRun()</script>"));
        }
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }
}

#[tokio::test]
async fn canvas_edits_preview_the_current_content_and_send_one_explicit_operation() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    let file = tempfile::NamedTempFile::new().unwrap();
    let markdown = "# Replacement\n$(do-not-execute) 🦀";
    fs::write(file.path(), markdown).unwrap();
    for (command, operation, section, has_body) in [
        ("append", "insert_at_end", None, true),
        ("prepend", "insert_at_start", None, true),
        (
            "insert-before",
            "insert_before",
            Some("temp:C:before"),
            true,
        ),
        ("insert-after", "insert_after", Some("temp:C:after"), true),
        ("replace-section", "replace", Some("temp:C:replace"), true),
        ("delete-section", "delete", Some("temp:C:delete"), false),
    ] {
        for dry_run in [true, false] {
            let server = MockServer::start().await;
            identity(&server).await;
            let before = "# Existing\nKeep the other sections.\u{001b}]52;hidden";
            Mock::given(method("POST"))
                .and(path("/api/canvases.getContent"))
                .and(body_json(
                    json!({"canvas_id": "F7", "content_type": "markdown"}),
                ))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(json!({"ok": true, "content": before})),
                )
                .expect(1)
                .mount(&server)
                .await;
            let mut change = json!({"operation": operation});
            if let Some(section) = section {
                change["section_id"] = json!(section);
            }
            if has_body {
                change["document_content"] = json!({"type": "markdown", "markdown": markdown});
            }
            let expected = json!({"canvas_id": "F7", "changes": [change]});
            Mock::given(method("POST"))
                .and(path("/api/canvases.edit"))
                .and(body_json(&expected))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
                .expect(u64::from(!dry_run))
                .mount(&server)
                .await;
            let mut args = vec![command, "F7"];
            if let Some(section) = section {
                args.push(section);
            }
            if has_body {
                args.extend(["--file", file.path().to_str().unwrap()]);
            }
            args.extend([
                "--profile",
                "work",
                if dry_run { "--dry-run" } else { "--yes" },
            ]);
            let result = execute(prepare(&args).unwrap(), &server, &credentials)
                .await
                .unwrap();
            let value = serde_json::to_value(&result).unwrap();
            assert_eq!(value["changes"], expected["changes"]);
            assert_eq!(value["canvas_id"], "F7");
            if dry_run {
                assert_eq!(value["before"], before);
                assert_eq!(value["dry_run"], true);
                assert_eq!(value["user_id"], "W87654321");
            } else {
                assert_eq!(value, expected);
            }
            let mut text = Vec::new();
            result.write(false, &mut text).unwrap();
            let text = String::from_utf8(text).unwrap();
            assert!(!text.contains('\x1b'));
            assert!(!text.contains("hidden"));
            if has_body {
                assert!(text.contains(markdown));
            }
            if dry_run {
                assert!(text.contains("Keep the other sections."));
                assert!(text.contains("does not lock the canvas"));
            }
            assert_eq!(
                server.received_requests().await.unwrap().len(),
                if dry_run { 2 } else { 3 }
            );
        }
    }
}

#[tokio::test]
async fn canvas_failures_preserve_read_write_and_permission_semantics_without_retries() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for (on_read, response, expected) in [
        (
            true,
            ResponseTemplate::new(200).set_body_json(json!({"ok": true})),
            "invalid_response",
        ),
        (
            true,
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok": false, "error": "canvas_not_found"})),
            "slack_error",
        ),
        (
            false,
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok": false, "error": "missing_scope"})),
            "slack_error",
        ),
        (
            false,
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok": false, "error": "canvas_editing_locked"})),
            "slack_error",
        ),
        (
            false,
            ResponseTemplate::new(200).set_body_json(
                json!({"ok": false, "error": "free_teams_cannot_edit_standalone_canvases"}),
            ),
            "slack_error",
        ),
        (
            false,
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok": false, "error": "use_skills_api"})),
            "slack_error",
        ),
        (
            false,
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok": false, "error": "internal_error"})),
            "write_outcome_unknown",
        ),
        (
            false,
            ResponseTemplate::new(200).set_body_json(json!({"unexpected": true})),
            "write_outcome_unknown",
        ),
        (
            false,
            ResponseTemplate::new(503).set_body_string("xoxp-test secret response"),
            "write_outcome_unknown",
        ),
        (
            false,
            ResponseTemplate::new(429)
                .insert_header("Retry-After", "19")
                .set_body_json(json!({"ok": false, "error": "ratelimited"})),
            "rate_limited",
        ),
        (
            true,
            ResponseTemplate::new(200).set_delay(Duration::from_secs(3)),
            "request_timeout",
        ),
        (
            false,
            ResponseTemplate::new(200).set_delay(Duration::from_secs(3)),
            "write_outcome_unknown",
        ),
    ] {
        let server = MockServer::start().await;
        identity(&server).await;
        Mock::given(path("/api/canvases.getContent"))
            .respond_with(if on_read {
                response.clone()
            } else {
                ResponseTemplate::new(200).set_body_json(json!({"ok": true, "content": "current"}))
            })
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(path("/api/canvases.edit"))
            .respond_with(response)
            .expect(u64::from(!on_read))
            .mount(&server)
            .await;
        let error = execute(
            prepare(&[
                "append",
                "F7",
                "--text",
                "new",
                "--profile",
                "work",
                "--yes",
            ])
            .unwrap(),
            &server,
            &credentials,
        )
        .await
        .err()
        .unwrap();
        assert_eq!(error.report().code, expected);
        assert!(
            !serde_json::to_string(&error.report())
                .unwrap()
                .contains("xoxp-test")
        );
        if expected == "rate_limited" {
            assert_eq!(error.report().retry_after, Some(19));
        }
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            if on_read { 2 } else { 3 }
        );
    }
}

#[tokio::test]
async fn a_changed_identity_cannot_read_or_edit_the_selected_canvas() {
    let credentials = Credentials::parse(Kind::User, r#"{"token":"xoxp-test"}"#).unwrap();
    for args in [
        vec!["read", "F7"],
        vec!["sections", "F7", "--contains", "Plan"],
        vec![
            "append",
            "F7",
            "--text",
            "new",
            "--profile",
            "work",
            "--yes",
        ],
    ] {
        let server = MockServer::start().await;
        let mut response = auth_response();
        response["team_id"] = json!("T999");
        Mock::given(path("/api/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .expect(1)
            .mount(&server)
            .await;
        let error = execute(prepare(&args).unwrap(), &server, &credentials)
            .await
            .err()
            .unwrap();
        assert_eq!(error.report().code, "identity_mismatch");
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[test]
fn canvas_inputs_require_a_bounded_body_and_explicit_sections_for_destructive_operations() {
    for args in [
        vec!["replace-section", "F7", "--text", "new", "--dry-run"],
        vec!["replace-section", "F7", "", "--text", "new", "--dry-run"],
        vec!["delete-section", "F7", "--dry-run"],
        vec!["delete-section", "F7", "temp:C:a\n", "--dry-run"],
        vec!["delete", "F7", "--dry-run"],
        vec!["replace", "F7", "--text", "new", "--dry-run"],
        vec!["append", "F7", "--text", " \n\t", "--dry-run"],
        vec!["append", "F7", "--text", "new", "--file", "-", "--dry-run"],
        vec!["sections", "F7"],
        vec!["sections", "F7", "--contains", ""],
        vec!["read", "https://team.slack.com/docs/F7"],
    ] {
        assert_eq!(
            prepare(&args).err().unwrap().report().code,
            "invalid_arguments",
            "{args:?}"
        );
    }
    let file = tempfile::NamedTempFile::new().unwrap();
    let mut content = "🦀".repeat(262_144);
    fs::write(file.path(), &content).unwrap();
    assert!(
        prepare(&[
            "append",
            "F7",
            "--file",
            file.path().to_str().unwrap(),
            "--dry-run"
        ])
        .is_ok()
    );
    content.push('x');
    fs::write(file.path(), content).unwrap();
    assert_eq!(
        prepare(&[
            "append",
            "F7",
            "--file",
            file.path().to_str().unwrap(),
            "--dry-run"
        ])
        .err()
        .unwrap()
        .report()
        .code,
        "invalid_arguments"
    );
}
