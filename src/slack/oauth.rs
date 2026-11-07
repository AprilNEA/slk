use super::{Slack, map_write_error};
use crate::auth::oauth::Authorization;
use crate::auth::{Credentials, Identity, Kind, Rotation};
use crate::error::{Error, Result};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use slack_morphism::{SlackClientApiCallContext, SlackClientHttpConnector};
use std::num::NonZeroU64;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Serialize)]
#[serde(tag = "grant_type", rename_all = "snake_case")]
enum Grant<'a> {
    AuthorizationCode {
        client_id: &'a str,
        code: &'a str,
        code_verifier: &'a str,
        redirect_uri: &'a str,
    },
    RefreshToken {
        client_id: &'a str,
        refresh_token: &'a str,
    },
}

#[derive(Deserialize)]
struct Token {
    access_token: SecretString,
    token_type: String,
    scope: String,
    refresh_token: Option<SecretString>,
    expires_in: Option<NonZeroU64>,
}

#[derive(Deserialize)]
struct GrantedUser {
    id: String,
    #[serde(flatten)]
    token: Token,
}

#[derive(Deserialize)]
struct GrantedTeam {
    id: String,
}

#[derive(Deserialize)]
struct GrantResponse {
    authed_user: GrantedUser,
    team: GrantedTeam,
}

impl<C: SlackClientHttpConnector + Send + Sync> Slack<C> {
    pub(crate) async fn oauth_exchange(
        &self,
        authorization: &Authorization,
    ) -> Result<(Credentials, Identity)> {
        let issued_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| Error::io("read the system clock", std::io::Error::other(error)))?
            .as_secs();
        let response: GrantResponse = self
            .oauth(&Grant::AuthorizationCode {
                client_id: &authorization.login.client_id,
                code: authorization.code.expose_secret(),
                code_verifier: authorization.verifier.expose_secret(),
                redirect_uri: authorization.login.redirect_uri.as_str(),
            })
            .await?;
        let credentials = response
            .authed_user
            .token
            .credentials(&authorization.login.client_id, issued_at)?
            .with_realm(authorization.login.workspace.realm())?;
        let identity = self.identify(&credentials).await?;
        if identity.workspace_url != authorization.login.workspace
            || identity.team_id != response.team.id
            || identity.user_id != response.authed_user.id
        {
            return Err(Error::IdentityMismatch);
        }
        Ok((credentials, identity))
    }

    pub(crate) async fn oauth_refresh(
        &self,
        credentials: &Credentials,
        identity: &Identity,
    ) -> Result<Credentials> {
        let rotation = credentials.rotation()?;
        let issued_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| Error::io("read the system clock", std::io::Error::other(error)))?
            .as_secs();
        let response: Token = self
            .oauth(&Grant::RefreshToken {
                client_id: &rotation.client_id,
                refresh_token: rotation.refresh_token.expose_secret(),
            })
            .await?;
        async {
            let refreshed = response
                .credentials(&rotation.client_id, issued_at)?
                .with_realm(credentials.realm())?;
            refreshed.rotation().map_err(|_| Error::Response)?;
            if &self.identify(&refreshed).await? != identity {
                return Err(Error::IdentityMismatch);
            }
            Ok(refreshed)
        }
        .await
        .map_err(|source| Error::RefreshNotSaved {
            source: Box::new(source),
        })
    }

    async fn oauth<T: DeserializeOwned + Send>(&self, grant: &Grant<'_>) -> Result<T> {
        let span = tracing::Span::none();
        let context = SlackClientApiCallContext {
            rate_control_params: None,
            token: None,
            tracing_span: &span,
            is_sensitive_url: true,
        };
        let body = serde_urlencoded::to_string(grant)
            .expect("OAuth grants contain only flat string fields");
        let connector = &self.client.http_api.connector;
        let endpoint = connector
            .create_method_uri_path("oauth.v2.access")
            .map_err(super::map_slack_error)?;
        self.response(connector.http_post_uri_binary(
            endpoint,
            "application/x-www-form-urlencoded".to_owned(),
            body.as_bytes(),
            context,
        ))
        .await
        .map_err(|error| match map_write_error(error) {
            Error::WriteOutcomeUnknown { .. } => Error::OAuthOutcomeUnknown,
            error => error,
        })
    }
}

impl Token {
    fn credentials(self, client_id: &str, issued_at: u64) -> Result<Credentials> {
        if self.token_type != "user" {
            return Err(Error::Response);
        }
        let rotation = match (self.refresh_token, self.expires_in) {
            (None, None) => None,
            (Some(refresh_token), Some(expires_in)) => Some(Rotation {
                client_id: client_id.to_owned(),
                refresh_token,
                expires_at: issued_at
                    .checked_add(expires_in.get())
                    .ok_or(Error::Response)?,
                scope: self.scope,
            }),
            _ => return Err(Error::Response),
        };
        Credentials::new(Kind::User, self.access_token, None, None)
            .and_then(|credentials| credentials.with_rotation(rotation))
            .map_err(|_| Error::Response)
    }
}
