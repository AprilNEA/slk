use super::{cancellation, compat, session::SessionSource};
use crate::auth::{Credentials, Kind, Realm, Store, Workspace, browser, oauth};
use crate::error::{Error, Result};
use crate::output::Output;
use crate::slack::Slack;
use clap::Subcommand;
use secrecy::SecretString;
use std::io::{self, IsTerminal, Read, Write};
use url::Url;
use zeroize::Zeroizing;

#[derive(Subcommand)]
pub(super) enum AuthCommand {
    /// Authorize a PKCE-enabled Slack app using explicit user scopes and a localhost callback.
    Oauth {
        /// Public Slack application client ID, not a client secret.
        #[arg(long)]
        client_id: String,
        /// Registered http://localhost:PORT/PATH callback on this machine.
        #[arg(long)]
        redirect_uri: Url,
        /// Expected HTTPS Slack workspace origin; a different workspace is rejected.
        #[arg(long)]
        workspace: String,
        /// Comma-separated user scopes configured for the app; no bot scopes are requested.
        #[arg(long, value_delimiter = ',', required = true)]
        scope: Vec<String>,
        /// Print the authorization URL on stderr instead of opening the system browser.
        #[arg(long, conflicts_with = "json")]
        no_open: bool,
    },
    /// Rotate this profile's PKCE credentials explicitly; never replay a business request.
    Refresh,
    /// Log in with a temporary Chrome profile (experimental; requires organizational approval).
    #[command(visible_alias = "login-auto")]
    Browser {
        /// Select exactly one HTTPS Slack workspace origin, such as https://team.slack.com.
        #[arg(long, visible_alias = "workspace-url")]
        workspace: String,
    },
    /// Verify credentials with auth.test and store them in the system keyring.
    Login {
        /// Authentication kind. Browser sessions are experimental and require organizational approval.
        #[arg(long, value_enum)]
        kind: Kind,
        /// Read credential JSON from stdin instead of hidden terminal prompts.
        #[arg(long)]
        stdin: bool,
        /// Token environment for hidden input; stdin JSON carries its own realm.
        #[arg(long, value_enum, conflicts_with = "stdin")]
        realm: Option<Realm>,
    },
    /// List local profiles without contacting Slack or reading credentials.
    List,
    /// Verify the selected profile's identity with Slack (not its scopes).
    #[command(visible_alias = "whoami")]
    Status {
        #[command(flatten)]
        selector: compat::Selector,
    },
    /// Delete local credentials; this does not revoke the Slack session.
    Logout,
}

impl AuthCommand {
    pub(super) async fn execute(
        self,
        store: &mut Store,
        profile: Option<String>,
        source: &SessionSource,
        json: bool,
    ) -> Result<Output> {
        let _lock = if matches!(&self, Self::List | Self::Status { .. }) {
            None
        } else {
            Some(store.lock()?)
        };
        match self {
            Self::Oauth {
                client_id,
                redirect_uri,
                workspace,
                scope,
                no_open,
            } => {
                let name = profile.as_deref().ok_or(Error::Arguments(
                    "OAuth login requires an explicit --profile.",
                ))?;
                let login = oauth::Login {
                    client_id,
                    redirect_uri,
                    scopes: scope,
                    workspace: Workspace::try_from(workspace)?,
                };
                let cancelled = cancellation()?;
                tokio::pin!(cancelled);
                let authorization = login
                    .authorize(store.settings(), cancelled.as_mut(), |url| {
                        if no_open {
                            writeln!(
                                io::stderr().lock(),
                                "Open this URL in a browser on this machine:\n{url}"
                            )
                            .map_err(|error| Error::io("write OAuth login instructions", error))
                        } else {
                            webbrowser::open(url.as_str()).map_err(|_| Error::OAuthBrowser)
                        }
                    })
                    .await?;
                let slack = Slack::connect(
                    authorization.login.workspace.realm(),
                    None,
                    store.settings(),
                )?;
                let (credentials, identity) = tokio::select! {
                    biased;
                    () = cancelled.as_mut() => return Err(Error::OAuthCancelled),
                    result = slack.oauth_exchange(&authorization) => result?,
                };
                store
                    .login(name, &credentials, identity)
                    .map(Output::Profile)
            }
            Self::Refresh => {
                let name = profile.as_deref().ok_or(Error::Arguments(
                    "Refreshing credentials requires an explicit --profile.",
                ))?;
                let profile = store.select(Some(name))?;
                let credentials = store.credentials(name)?;
                credentials.rotation()?;
                let slack = Slack::connect(credentials.realm(), None, store.settings())?;
                let cancelled = cancellation()?;
                let refreshed = tokio::select! {
                    biased;
                    () = cancelled => return Err(Error::OAuthOutcomeUnknown),
                    result = slack.oauth_refresh(&credentials, &profile.identity) => result?,
                };
                store
                    .login(name, &refreshed, profile.identity)
                    .map(Output::Profile)
                    .map_err(|source| Error::RefreshNotSaved {
                        source: Box::new(source),
                    })
            }
            Self::Browser { workspace } => {
                let name = profile.as_deref().ok_or(Error::Arguments(
                    "Browser login requires an explicit --profile.",
                ))?;
                let workspace = Workspace::try_from(workspace)?;
                let cancelled = cancellation()?;
                tokio::pin!(cancelled);
                if !json {
                    writeln!(io::stderr().lock(),
                        "Complete sign-in and SSO/MFA for {workspace} in the temporary browser. Press Ctrl+C to cancel.")
                        .map_err(|error| Error::io("write browser-login instructions", error))?;
                }
                let credentials =
                    browser::acquire(workspace, store.settings(), cancelled.as_mut()).await?;
                let slack = Slack::connect(
                    credentials.realm(),
                    credentials.workspace(),
                    store.settings(),
                )?;
                let identity = tokio::select! {
                    biased;
                    () = cancelled.as_mut() => return Err(Error::BrowserCancelled),
                    identity = slack.identify(&credentials) => identity?,
                };
                store
                    .login(name, &credentials, identity)
                    .map(Output::Profile)
            }
            Self::Login { kind, stdin, realm } => {
                let name = profile
                    .as_deref()
                    .ok_or(Error::Arguments("Login requires an explicit --profile."))?;
                let credentials = read_credentials(kind, stdin, realm)?;
                let slack = Slack::connect(
                    credentials.realm(),
                    credentials.workspace(),
                    store.settings(),
                )?;
                let identity = slack.identify(&credentials).await?;
                store
                    .login(name, &credentials, identity)
                    .map(Output::Profile)
            }
            Self::List => {
                if profile.is_some() && !source.injected() {
                    return Err(Error::Arguments(
                        "auth list lists all profiles; omit --profile.",
                    ));
                }
                Ok(Output::Profiles {
                    data: store.profiles(),
                })
            }
            Self::Status { selector } => {
                let selected = selector.resolve(store, profile.as_deref())?;
                let profile = store.select(selected.as_deref())?;
                let credentials = store.credentials(&profile.profile)?;
                let slack = Slack::connect(
                    credentials.realm(),
                    credentials.workspace(),
                    store.settings(),
                )?;
                let identity = slack.identify(&credentials).await?;
                if identity != profile.identity {
                    return Err(Error::IdentityMismatch);
                }
                Ok(Output::Profile(profile))
            }
            Self::Logout => {
                let name =
                    profile.ok_or(Error::Arguments("Logout requires an explicit --profile."))?;
                store.logout(&name)?;
                Ok(Output::LoggedOut {
                    profile: name,
                    local_credentials_removed: true,
                })
            }
        }
    }
}

fn read_credentials(kind: Kind, stdin: bool, realm: Option<Realm>) -> Result<Credentials> {
    if stdin {
        let mut input = Zeroizing::new(String::new());
        io::stdin()
            .read_to_string(&mut input)
            .map_err(|error| Error::io("read credential JSON from stdin", error))?;
        return Credentials::parse(kind, &input);
    }
    if !io::stdin().is_terminal() {
        return Err(Error::Arguments(
            "Non-interactive login requires --stdin and credential JSON.",
        ));
    }
    let workspace = if kind == Kind::Browser {
        Some(Workspace::try_from(
            rpassword::prompt_password("Workspace URL (hidden): ")
                .map_err(|error| Error::io("read the workspace URL from the terminal", error))?,
        )?)
    } else {
        None
    };
    let token = SecretString::from(
        rpassword::prompt_password("Token (hidden): ")
            .map_err(|error| Error::io("read the token from the terminal", error))?,
    );
    let cookie = if kind == Kind::Browser {
        Some(SecretString::from(
            rpassword::prompt_password("Cookie d value (hidden): ")
                .map_err(|error| Error::io("read the cookie from the terminal", error))?,
        ))
    } else {
        None
    };
    let credentials = Credentials::new(kind, token, workspace, cookie)?;
    match realm {
        Some(realm) => credentials.with_realm(realm),
        None => Ok(credentials),
    }
}
