use super::Paging;
use super::write::WriteOptions;
use crate::app::{Approval, Connection, Prompt};
use crate::auth::{ProfileView, Store};
use crate::error::{Error, Result};
use crate::ids::parse_user;
use crate::output::{self, Output};
use crate::slack::CursorPage;
use clap::{Subcommand, ValueEnum};
use slack_morphism::{SlackClientHttpConnector, SlackUserId};

#[derive(Subcommand)]
pub(super) enum UserCommand {
    /// List or inspect users visible to the selected identity.
    Users {
        #[command(flatten)]
        selector: super::compat::Selector,
        #[command(subcommand)]
        command: UserQuery,
    },
    /// Find exact name candidates; one page by default, never an implicit target.
    Resolve {
        #[arg(value_enum)]
        kind: ResolveKind,
        /// User name, display name, real name, or channel name; @ and # are optional.
        name: String,
        #[command(flatten)]
        paging: Paging,
    },
    /// Explicitly open or create a single-person DM, without sending a message.
    Dm {
        #[arg(value_parser = parse_user)]
        user: SlackUserId,
        #[command(flatten)]
        options: WriteOptions,
    },
}

#[derive(Subcommand)]
pub(super) enum UserQuery {
    /// List users, including deactivated accounts and bots; one page by default.
    List(Paging),
    /// Read one user by ID.
    Info {
        #[arg(value_parser = parse_user)]
        user: SlackUserId,
    },
    /// Look up a user by email; requires users:read.email.
    Lookup { email: String },
}

#[derive(Clone, Copy, ValueEnum)]
pub(super) enum ResolveKind {
    User,
    Channel,
}

enum Request {
    List(CursorPage),
    Info(SlackUserId),
    Lookup(String),
    Resolve {
        kind: ResolveKind,
        name: String,
        page: CursorPage,
    },
    Dm {
        user: SlackUserId,
        approval: Approval,
    },
}

pub(super) struct Prepared {
    request: Request,
}

impl UserCommand {
    pub(super) fn prepare(
        self,
        store: &Store,
        selected: Option<&str>,
        json: bool,
    ) -> Result<(ProfileView, Prepared)> {
        let selection = match &self {
            Self::Users { selector, .. } => selector.resolve(store, selected)?,
            _ => selected.map(str::to_owned),
        };
        let selected = selection.as_deref();
        let profile = store.select(selected)?;
        let request = match self {
            Self::Users { command, .. } => match command {
                UserQuery::List(paging) => Request::List(paging.resolve(store.settings())),
                UserQuery::Info { user } => Request::Info(user),
                UserQuery::Lookup { email } => {
                    if email.trim().is_empty() || email.chars().any(char::is_control) {
                        return Err(Error::Arguments(
                            "Provide a nonempty email address without controls.",
                        ));
                    }
                    Request::Lookup(email)
                }
            },
            Self::Resolve { kind, name, paging } => {
                let prefix = match kind {
                    ResolveKind::User => '@',
                    ResolveKind::Channel => '#',
                };
                let name = name.strip_prefix(prefix).unwrap_or(&name).to_owned();
                if name.trim().is_empty() || name.chars().any(char::is_control) {
                    return Err(Error::Arguments(
                        "Provide a nonempty name without controls.",
                    ));
                }
                Request::Resolve {
                    kind,
                    name,
                    page: paging.resolve(store.settings()),
                }
            }
            Self::Dm { user, options } => Request::Dm {
                user,
                approval: options.prepare(selected.is_some(), json)?,
            },
        };
        Ok((profile, Prepared { request }))
    }
}

impl Prepared {
    pub(super) async fn execute<C: SlackClientHttpConnector + Send + Sync>(
        self,
        connection: Connection<C>,
        prompt: &dyn Prompt,
    ) -> Result<Output> {
        let Connection {
            slack,
            credentials,
            profile,
        } = connection;
        let credentials = &credentials;
        match self.request {
            Request::List(page) => slack.users(credentials, &page).await.map(Output::Users),
            Request::Info(user) => slack.user(credentials, &user).await.map(Output::User),
            Request::Lookup(email) => slack
                .user_by_email(credentials, &email)
                .await
                .map(Output::User),
            Request::Resolve { kind, name, page } => match kind {
                ResolveKind::User => {
                    let mut candidates = slack.users(credentials, &page).await?;
                    candidates.data.retain(|user| user.matches_name(&name));
                    Ok(Output::ResolvedUsers(candidates.resolve(&page)))
                }
                ResolveKind::Channel => {
                    let mut candidates = slack.channels(credentials, &page).await?;
                    candidates.data.retain(|channel| {
                        channel
                            .name
                            .as_ref()
                            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(&name))
                    });
                    Ok(Output::ResolvedChannels(candidates.resolve(&page)))
                }
            },
            Request::Dm { user, approval } => {
                let target = slack.user(credentials, &user).await?;
                if target.deleted || target.is_bot || target.is_app_user {
                    return Err(Error::DmTarget);
                }
                if !approval.confirm(prompt, "Open or create this DM?", || {
                    output::dm_preview_text(&profile, &target)
                })? {
                    return Ok(Output::DmPreview {
                        dry_run: true,
                        profile,
                        target,
                    });
                }
                slack
                    .open_dm(credentials, &target.user_id)
                    .await
                    .map(Output::DmOpened)
            }
        }
    }
}

#[cfg(test)]
mod tests;
