use super::{Credentials, Kind, Settings, Workspace};
use crate::error::{Error, Result};
use crate::platform::{Native, Platform};
use bytes::Bytes;
use chromiumoxide::Page;
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::network::{
    EventRequestWillBeSent, GetCookiesParams, GetRequestPostDataParams,
};
use futures_util::{StreamExt, stream};
use secrecy::SecretString;
use serde::Deserialize;
use std::convert::Infallible;
use std::future::Future;
use std::time::Duration;
use tempfile::TempDir;
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout, timeout_at};
use url::Url;
use zeroize::Zeroizing;

struct Session {
    browser: Browser,
    handler: JoinHandle<()>,
    directory: TempDir,
    timeout: Duration,
}

pub(crate) async fn acquire(
    workspace: Workspace,
    settings: &Settings,
    cancelled: impl Future<Output = ()>,
) -> Result<Credentials> {
    let deadline = Instant::now() + Duration::from_secs(settings.login_timeout_secs.get());
    let session = Session::launch(settings, deadline).await?;
    session.acquire(workspace, deadline, cancelled).await
}

impl Session {
    async fn launch(settings: &Settings, deadline: Instant) -> Result<Self> {
        crate::initialize_tls();
        let directory = tempfile::Builder::new()
            .prefix("slk-login-")
            .tempdir()
            .map_err(|error| Error::io("create the temporary browser profile", error))?;
        Native::restrict(directory.path(), true)
            .map_err(|error| Error::io("secure the temporary browser profile", error))?;
        let timeout = Duration::from_secs(settings.request_timeout_secs.get());
        let mut builder = BrowserConfig::builder()
            .with_head()
            .respect_https_errors()
            .disable_default_args()
            .args([
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-sync",
            ])
            .arg("--remote-debugging-address=127.0.0.1")
            .viewport(None)
            .user_data_dir(directory.path())
            .request_timeout(timeout)
            .launch_timeout(timeout.min(Duration::from_secs(settings.login_timeout_secs.get())));
        if let Some(executable) = &settings.browser.executable {
            builder = builder.chrome_executable(executable);
        }
        let launched = async {
            let config = builder.build().map_err(|_| Error::BrowserUnavailable)?;
            Browser::launch(config)
                .await
                .map_err(|_| Error::BrowserStart)
        }
        .await;
        let (browser, mut handler) = match launched {
            Ok(launched) => launched,
            Err(error) => {
                directory.close().map_err(|_| Error::BrowserCleanup)?;
                return Err(error);
            }
        };
        let handler = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if event.is_err() {
                    break;
                }
            }
        });
        let mut session = Self {
            browser,
            handler,
            directory,
            timeout,
        };
        let initialized = timeout_at(deadline, session.browser.start_incognito_context()).await;
        let error = match initialized {
            Ok(Ok(_)) => return Ok(session),
            Ok(Err(_)) => Error::BrowserControl,
            Err(_) => Error::BrowserTimeout,
        };
        session.close().await?;
        Err(error)
    }

    async fn acquire(
        self,
        workspace: Workspace,
        deadline: Instant,
        cancelled: impl Future<Output = ()>,
    ) -> Result<Credentials> {
        let result = tokio::select! {
            biased;
            () = cancelled => Err(Error::BrowserCancelled),
            result = timeout_at(deadline, async {
                let page = self.browser.new_page("about:blank").await.map_err(|_| Error::BrowserControl)?;
                capture(&page, workspace).await
            }) => result.unwrap_or(Err(Error::BrowserTimeout)),
        };
        self.close().await?;
        result
    }

    async fn close(mut self) -> Result<()> {
        let stopped: Result<()> = async {
            if self
                .browser
                .try_wait()
                .map_err(|_| Error::BrowserCleanup)?
                .is_none()
            {
                let graceful = timeout(self.timeout, async {
                    self.browser
                        .close()
                        .await
                        .map_err(|_| Error::BrowserCleanup)?;
                    self.browser.wait().await.map_err(|_| Error::BrowserCleanup)
                })
                .await;
                if !matches!(graceful, Ok(Ok(_))) {
                    self.browser
                        .kill()
                        .await
                        .ok_or(Error::BrowserCleanup)?
                        .map_err(|_| Error::BrowserCleanup)?;
                }
            }
            Ok(())
        }
        .await;
        self.handler.abort();
        let handled = match self.handler.await {
            Ok(()) => Ok(()),
            Err(error) if error.is_cancelled() => Ok(()),
            Err(_) => Err(Error::BrowserCleanup),
        };
        drop(self.browser);
        let removed = self.directory.close().map_err(|_| Error::BrowserCleanup);
        stopped?;
        handled?;
        removed
    }
}

async fn capture(page: &Page, workspace: Workspace) -> Result<Credentials> {
    let mut requests = page
        .event_listener::<EventRequestWillBeSent>()
        .await
        .map_err(|_| Error::BrowserControl)?;
    page.goto(workspace.to_string())
        .await
        .map_err(|_| Error::BrowserControl)?;
    let api_url = Url::parse(&workspace.api_url()).map_err(|_| Error::Workspace)?;
    while let Some(event) = requests.next().await {
        let Ok(url) = Url::parse(&event.request.url) else {
            continue;
        };
        if url.origin() != api_url.origin()
            || !url.path().starts_with("/api/")
            || event.request.has_post_data != Some(true)
        {
            continue;
        }
        let Some(content_type) = event
            .request
            .headers
            .inner()
            .as_object()
            .and_then(|headers| {
                headers
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            })
            .and_then(|(_, value)| value.as_str())
        else {
            continue;
        };
        let content_type: mime::Mime = content_type.parse().map_err(|_| Error::BrowserCapture)?;
        let body = async {
            let body = page
                .execute(GetRequestPostDataParams::new(event.request_id.clone()))
                .await
                .map_err(|_| Error::BrowserCapture)?
                .result
                .post_data;
            Ok::<_, Error>(Bytes::from_owner(Zeroizing::new(body.into_bytes())))
        };
        let token = match content_type.essence_str() {
            "application/json" => {
                #[derive(Deserialize)]
                struct Input {
                    token: Option<SecretString>,
                }
                serde_json::from_slice::<Input>(&body.await?)
                    .map_err(|_| Error::BrowserCapture)?
                    .token
            }
            "application/x-www-form-urlencoded" => url::form_urlencoded::parse(&body.await?)
                .find(|(name, _)| name == "token")
                .map(|(_, value)| SecretString::from(value.into_owned())),
            "multipart/form-data" => {
                let boundary = content_type
                    .get_param(mime::BOUNDARY)
                    .ok_or(Error::BrowserCapture)?;
                let body = body.await?;
                let mut multipart = multer::Multipart::new(
                    stream::once(async { Ok::<_, Infallible>(body) }),
                    boundary.as_str(),
                );
                let mut token = None;
                while let Some(field) = multipart
                    .next_field()
                    .await
                    .map_err(|_| Error::BrowserCapture)?
                {
                    if field.name() == Some("token") {
                        token = Some(SecretString::from(
                            field.text().await.map_err(|_| Error::BrowserCapture)?,
                        ));
                        break;
                    }
                }
                token
            }
            _ => None,
        };
        let Some(token) = token else { continue };
        let cookies = page
            .execute(
                GetCookiesParams::builder()
                    .url(format!("{}/auth.test", workspace.api_url()))
                    .build(),
            )
            .await
            .map_err(|_| Error::BrowserCapture)?
            .result
            .cookies;
        if let Some(cookie) = cookies.into_iter().find(|cookie| cookie.name == "d") {
            return Credentials::new(
                Kind::Browser,
                token,
                Some(workspace),
                Some(SecretString::from(cookie.value)),
            );
        }
    }
    Err(Error::BrowserClosed)
}

#[cfg(test)]
mod tests;
