use super::credentials::valid_client_id;
use super::{Settings, Workspace};
use crate::error::{Error, Result};
use axum::Router;
use axum::extract::{Query, State, rejection::QueryRejection};
use axum::http::{Method, StatusCode, Uri, header};
use axum::response::IntoResponse;
use oauth2::basic::BasicClient;
use oauth2::{AuthUrl, ClientId, CsrfToken, PkceCodeChallenge, RedirectUrl};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use std::future::{Future, IntoFuture};
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, oneshot};
use url::Url;

pub(crate) struct Login {
    pub client_id: String,
    pub redirect_uri: Url,
    pub scopes: Vec<String>,
    pub workspace: Workspace,
}

pub(crate) struct Authorization {
    pub login: Login,
    pub code: SecretString,
    pub verifier: SecretString,
}

struct CallbackState {
    path: String,
    csrf: CsrfToken,
    sender: Mutex<Option<oneshot::Sender<Result<SecretString>>>>,
}

#[derive(Deserialize)]
struct Callback {
    code: Option<SecretString>,
    state: Option<CsrfToken>,
    error: Option<String>,
}

impl Login {
    pub(crate) async fn authorize(
        self,
        settings: &Settings,
        cancelled: impl Future<Output = ()>,
        open: impl FnOnce(&Url) -> Result<()>,
    ) -> Result<Authorization> {
        if !valid_client_id(&self.client_id)
            || self.scopes.is_empty()
            || self.scopes.iter().any(|scope| {
                scope.is_empty()
                    || !scope
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b":._-".contains(&byte))
            })
        {
            return Err(Error::Arguments(
                "OAuth requires a numeric Slack client ID and explicit comma-separated user scopes.",
            ));
        }
        if self.redirect_uri.scheme() != "http"
            || self.redirect_uri.host_str() != Some("localhost")
            || !self.redirect_uri.username().is_empty()
            || self.redirect_uri.password().is_some()
            || self.redirect_uri.query().is_some()
            || self.redirect_uri.fragment().is_some()
        {
            return Err(Error::Arguments(
                "PKCE requires a registered http://localhost:PORT/PATH redirect URI without credentials, query, or fragment.",
            ));
        }
        let port = self
            .redirect_uri
            .port()
            .filter(|port| *port != 0)
            .ok_or(Error::Arguments(
                "The OAuth redirect URI must name a nonzero local port.",
            ))?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
            .await
            .map_err(|error| Error::io("listen for the local OAuth callback", error))?;
        let client = BasicClient::new(ClientId::new(self.client_id.clone()))
            .set_auth_uri(
                AuthUrl::new(self.workspace.realm().authorization_url())
                    .expect("the Slack authorization endpoint is a static HTTPS URL"),
            )
            .set_redirect_uri(RedirectUrl::from_url(self.redirect_uri.clone()));
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let (authorization_url, csrf) = client
            .authorize_url(CsrfToken::new_random)
            .set_pkce_challenge(challenge)
            .add_extra_param("user_scope", self.scopes.join(","))
            .url();
        let (sender, receiver) = oneshot::channel();
        let app = Router::new()
            .fallback(callback)
            .with_state(Arc::new(CallbackState {
                path: self.redirect_uri.path().to_owned(),
                csrf,
                sender: Mutex::new(Some(sender)),
            }));
        let (stop, stopped) = oneshot::channel::<()>();
        let server = axum::serve(listener, app)
            .with_graceful_shutdown(async {
                match stopped.await {
                    Ok(()) | Err(_) => (),
                }
            })
            .into_future();
        tokio::pin!(server);
        open(&authorization_url)?;
        let result = tokio::select! {
            biased;
            () = cancelled => Err(Error::OAuthCancelled),
            result = receiver => result.map_err(|_| Error::OAuthCallback).and_then(|result| result),
            () = tokio::time::sleep(Duration::from_secs(settings.login_timeout_secs.get())) => Err(Error::OAuthTimeout),
            _ = server.as_mut() => return Err(Error::OAuthCallback),
        };
        drop(stop);
        tokio::time::timeout(
            Duration::from_secs(settings.request_timeout_secs.get()),
            server,
        )
        .await
        .map_err(|_| Error::OAuthCallback)?
        .map_err(|_| Error::OAuthCallback)?;
        Ok(Authorization {
            login: self,
            code: result?,
            verifier: SecretString::from(verifier.into_secret()),
        })
    }
}

async fn callback(
    State(state): State<Arc<CallbackState>>,
    method: Method,
    uri: Uri,
    query: std::result::Result<Query<Callback>, QueryRejection>,
) -> impl IntoResponse {
    let result = async {
        if method != Method::GET || uri.path() != state.path {
            return (StatusCode::NOT_FOUND, "No OAuth callback at this address.");
        }
        let Ok(Query(input)) = query else {
            return (StatusCode::BAD_REQUEST, "Invalid OAuth callback.");
        };
        if input.state.as_ref() != Some(&state.csrf) {
            return (
                StatusCode::FORBIDDEN,
                "Invalid OAuth state. Return to the original authorization page.",
            );
        }
        let result = match (input.code, input.error) {
            (Some(code), None) if !code.expose_secret().is_empty() => Ok(code),
            (None, Some(_)) => Err(Error::OAuthDenied),
            _ => return (StatusCode::BAD_REQUEST, "Invalid OAuth callback."),
        };
        let Some(sender) = state.sender.lock().await.take() else {
            return (
                StatusCode::GONE,
                "The OAuth callback has already been used.",
            );
        };
        match sender.send(result) {
            Ok(()) => (
                StatusCode::OK,
                "Authorization response received. Return to slk to check whether login succeeded.",
            ),
            Err(_) => (
                StatusCode::GONE,
                "slk is no longer waiting for authorization.",
            ),
        }
    }
    .await;
    (
        result.0,
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        result.1,
    )
}

#[cfg(test)]
mod tests;
