use crate::auth::{Credentials, Identity, Kind, Realm, Settings, Workspace};
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use slack_morphism::api::SlackApiAuthTestResponse;
use slack_morphism::errors::SlackClientError;
use slack_morphism::hyper_tokio::SlackClientHyperHttpsConnector;
use slack_morphism::{ClientResult, SlackApiToken, SlackClient, SlackClientHttpConnector};
use std::future::Future;
use std::time::Duration;

mod canvas;
mod conversations;
pub(crate) mod files;
pub(crate) mod messages;
mod oauth;
mod pagination;
mod scheduled;
mod users;

const CONVERSATION_TYPES: &str = "public_channel,private_channel,mpim,im";

pub(crate) struct Slack<C: SlackClientHttpConnector + Send> {
    client: SlackClient<C>,
    timeout: Duration,
}

#[derive(Deserialize)]
struct ApiResponse<T> {
    ok: bool,
    #[serde(flatten)]
    body: T,
}

impl Slack<SlackClientHyperHttpsConnector> {
    pub(crate) fn connect(
        realm: Realm,
        workspace: Option<&Workspace>,
        settings: &Settings,
    ) -> Result<Self> {
        crate::initialize_tls();
        if workspace.is_some_and(|workspace| workspace.realm() != realm) {
            return Err(Error::IdentityMismatch);
        }
        let endpoint = workspace.map_or_else(|| realm.api_url(), Workspace::api_url);
        let connector = SlackClientHyperHttpsConnector::new()
            .map_err(|_| Error::Transport)?
            .with_slack_api_url(&endpoint);
        Ok(Self::new(
            connector,
            Duration::from_secs(settings.request_timeout_secs.get()),
        ))
    }
}

impl<C: SlackClientHttpConnector + Send + Sync> Slack<C> {
    pub(crate) fn new(connector: C, timeout: Duration) -> Self {
        Self {
            client: SlackClient::new(connector),
            timeout,
        }
    }
}

impl<C: SlackClientHttpConnector + Send + Sync> Slack<C> {
    pub(crate) async fn identify(&self, credentials: &Credentials) -> Result<Identity> {
        let token = SlackApiToken::from(credentials);
        let session = self.client.open_session(&token);
        let body = serde_json::json!({});
        let response: SlackApiAuthTestResponse = self
            .response(session.http_session_api.http_post("auth.test", &body, None))
            .await?;
        if (credentials.kind() == Kind::Bot) != response.bot_id.is_some() {
            return Err(Error::BotIdentity);
        }
        let identity = Identity {
            workspace_url: Workspace::try_from(response.url.0.to_string())
                .map_err(|_| Error::Response)?,
            team_id: response.team_id.to_string(),
            user_id: response.user_id.to_string(),
            bot_id: response.bot_id.map(|id| id.to_string()),
        };
        identity.validate()?;
        if credentials.realm() != identity.workspace_url.realm()
            || credentials
                .workspace()
                .is_some_and(|workspace| workspace != &identity.workspace_url)
        {
            return Err(Error::IdentityMismatch);
        }
        Ok(identity)
    }

    async fn get<T: DeserializeOwned + Send>(
        &self,
        credentials: &Credentials,
        method: &str,
        params: Vec<(&str, Option<&str>)>,
    ) -> Result<T> {
        let token = SlackApiToken::from(credentials);
        let session = self.client.open_session(&token);
        self.response(session.http_session_api.http_get(method, &params, None))
            .await
    }

    async fn write<T: DeserializeOwned + Send>(
        &self,
        credentials: &Credentials,
        method: &str,
        body: &(impl Serialize + Send + Sync),
    ) -> Result<T> {
        self.post(credentials, method, body)
            .await
            .map_err(map_write_error)
    }

    async fn post<T: DeserializeOwned + Send>(
        &self,
        credentials: &Credentials,
        method: &str,
        body: &(impl Serialize + Send + Sync),
    ) -> Result<T> {
        let token = SlackApiToken::from(credentials);
        let session = self.client.open_session(&token);
        self.response(session.http_session_api.http_post(method, body, None))
            .await
    }

    async fn response<T>(
        &self,
        request: impl Future<Output = ClientResult<ApiResponse<T>>>,
    ) -> Result<T> {
        let response = tokio::time::timeout(self.timeout, request)
            .await
            .map_err(|_| Error::Timeout)?
            .map_err(map_slack_error)?;
        if !response.ok {
            return Err(Error::Response);
        }
        Ok(response.body)
    }
}

fn map_write_error(error: Error) -> Error {
    match error {
        Error::Transport | Error::Timeout | Error::Response => {
            Error::WriteOutcomeUnknown { slack_error: None }
        }
        Error::Http { status } if status >= 500 || status == 408 => {
            Error::WriteOutcomeUnknown { slack_error: None }
        }
        Error::Slack { slack_error }
            if matches!(
                slack_error.as_deref(),
                Some("internal_error" | "fatal_error" | "service_unavailable" | "request_timeout")
            ) =>
        {
            Error::WriteOutcomeUnknown { slack_error }
        }
        error => error,
    }
}

fn map_slack_error(error: SlackClientError) -> Error {
    match error {
        SlackClientError::ApiError(error) => Error::Slack {
            slack_error: safe_error_code(error.code),
        },
        SlackClientError::RateLimitError(error) => Error::RateLimited {
            retry_after: error.retry_after.map(|duration| duration.as_secs()),
            slack_error: error.code.and_then(safe_error_code),
        },
        SlackClientError::HttpError(error) => Error::Http {
            status: error.status_code.as_u16(),
        },
        SlackClientError::ProtocolError(_) => Error::Response,
        _ => Error::Transport,
    }
}

fn safe_error_code(code: String) -> Option<String> {
    (!code.is_empty()
        && code
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_'))
    .then_some(code)
}

#[cfg(test)]
mod tests;
