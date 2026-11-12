//! Fixtures shared by the crate's unit tests.

use crate::app::{Connection, Prompt, Request};
use crate::auth::{Credentials, Kind, ProfileView, Store};
use crate::error::Result;
use crate::output::Output;
use crate::slack::Slack;
use hyper_util::client::legacy::connect::HttpConnector;
use serde_json::{Map, Value, json};
use slack_morphism::hyper_tokio::SlackClientHyperConnector;
use std::time::Duration;
use tempfile::TempDir;
use wiremock::MockServer;

pub(crate) fn client(server: &MockServer) -> Slack<SlackClientHyperConnector<HttpConnector>> {
    client_at(&server.uri())
}

/// Builds a client for the mock server at `uri`, for callers that cannot borrow the server.
pub(crate) fn client_at(uri: &str) -> Slack<SlackClientHyperConnector<HttpConnector>> {
    Slack::new(
        SlackClientHyperConnector::with_connector(HttpConnector::new())
            .with_slack_api_url(&format!("{uri}/api")),
        Duration::from_secs(2),
    )
}

pub(crate) fn auth_response() -> Value {
    json!({
        "ok": true,
        "url": "https://team.slack.com/",
        "team": "Synthetic team",
        "team_id": "T12345678",
        "user_id": "W87654321"
    })
}

/// Loads a persistent store whose profiles all bind `kind` to the identity in [`auth_response`].
///
/// The store reads its configuration from the returned directory.
pub(crate) fn store(names: &[&str], kind: Kind, settings: Value) -> (TempDir, Store) {
    let identity = auth_response();
    let profile = json!({
        "kind": kind,
        "identity": {
            "workspace_url": identity["url"],
            "team_id": identity["team_id"],
            "user_id": identity["user_id"],
            "bot_id": (kind == Kind::Bot).then_some("B7"),
        },
        "keyring_ref": "00112233-4455-4677-8899-aabbccddeeff",
    });
    let profiles: Map<String, Value> = names
        .iter()
        .map(|name| ((*name).to_owned(), profile.clone()))
        .collect();
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("config.json"),
        json!({"settings": settings, "profiles": profiles}).to_string(),
    )
    .unwrap();
    let store = Store::load(directory.path().to_owned()).unwrap();
    (directory, store)
}

/// Fails a test that reaches the confirmation prompt: tests choose `--yes` or `--dry-run`.
struct Unasked;

impl Prompt for Unasked {
    fn confirm(&self, _question: &str, _preview: &str) -> Result<bool> {
        panic!("a unit test must not reach the confirmation prompt")
    }
}

/// Runs a prepared request against `server` after verifying `credentials` against the profile.
pub(crate) async fn execute(
    (profile, request): (ProfileView, Request),
    server: &MockServer,
    credentials: &Credentials,
) -> Result<Output> {
    let connection = Connection::verify(client(server), credentials.clone(), profile).await?;
    request.execute(connection, &Unasked).await
}
