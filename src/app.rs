//! Identity-verified Slack connections and the approval of writes.

use crate::auth::{Credentials, ProfileView, Store};
use crate::error::{Error, Result};
use crate::platform::{Native, Platform};
use crate::slack::Slack;
use slack_morphism::SlackClientHttpConnector;
use slack_morphism::hyper_tokio::SlackClientHyperHttpsConnector;
use std::future::Future;

/// A Slack client whose credentials are verified to belong to the selected profile.
pub(crate) struct Connection<C: SlackClientHttpConnector + Send> {
    pub(crate) slack: Slack<C>,
    pub(crate) credentials: Credentials,
    pub(crate) profile: ProfileView,
}

impl<C: SlackClientHttpConnector + Send + Sync> Connection<C> {
    /// Rejects credentials whose Slack identity differs from the identity saved in `profile`.
    pub(crate) async fn verify(
        slack: Slack<C>,
        credentials: Credentials,
        profile: ProfileView,
    ) -> Result<Self> {
        if slack.identify(&credentials).await? != profile.identity {
            return Err(Error::IdentityMismatch);
        }
        Ok(Self {
            slack,
            credentials,
            profile,
        })
    }

    pub(crate) fn into_profile(self) -> ProfileView {
        self.profile
    }
}

impl Connection<SlackClientHyperHttpsConnector> {
    /// Reads the credentials of `profile` from `store`, connects to Slack, and verifies the identity.
    pub(crate) async fn open(store: &Store, profile: ProfileView) -> Result<Self> {
        let credentials = store.credentials(&profile.profile)?;
        let slack = Slack::connect(
            credentials.realm(),
            credentials.workspace(),
            store.settings(),
        )?;
        Self::verify(slack, credentials, profile).await
    }
}

/// How a write proceeds after its target is verified.
pub(crate) enum Approval {
    /// Return the preview without making a write request.
    Preview,
    /// Show the preview and ask before writing.
    Confirm,
    /// Write without asking.
    Approved,
}

/// Shows a write preview to a person and asks whether the write may proceed.
pub(crate) trait Prompt: Sync {
    fn confirm(&self, question: &str, preview: &str) -> Result<bool>;
}

impl Approval {
    /// Returns whether to write. A refused confirmation is [`Error::Cancelled`].
    pub(crate) fn confirm(
        self,
        prompt: &dyn Prompt,
        question: &str,
        preview: impl FnOnce() -> String,
    ) -> Result<bool> {
        match self {
            Self::Preview => Ok(false),
            Self::Approved => Ok(true),
            Self::Confirm => {
                if prompt.confirm(question, &preview())? {
                    Ok(true)
                } else {
                    Err(Error::Cancelled)
                }
            }
        }
    }
}

/// Resolves when the process receives an interrupt or termination signal.
pub(crate) fn cancellation() -> Result<impl Future<Output = ()>> {
    Native::cancellation().map_err(|error| Error::io("handle cancellation signals", error))
}
