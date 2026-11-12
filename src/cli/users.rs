use super::write::WriteOptions;
use super::{Effect, FilePath, Paging};
use crate::app::{self, conversations, users::Request};
use crate::auth::{ProfileView, Store};
use crate::error::{Error, Result};
use crate::ids::parse_user;
use clap::{Subcommand, ValueEnum};
use slack_morphism::SlackUserId;

#[derive(Subcommand)]
pub(super) enum UserCommand {
    /// List or inspect users visible to the selected identity.
    Users {
        #[command(flatten)]
        selector: super::store::Selector,
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

impl UserCommand {
    pub(super) fn effect(&self) -> Effect {
        match self {
            Self::Users { command, .. } => match command {
                UserQuery::List(_) | UserQuery::Info { .. } | UserQuery::Lookup { .. } => {
                    Effect::Read
                }
            },
            Self::Resolve { .. } => Effect::Read,
            Self::Dm { .. } => Effect::Write,
        }
    }

    pub(super) fn file_paths(&mut self) -> Vec<FilePath<'_>> {
        match self {
            Self::Users { command, .. } => match command {
                UserQuery::List(_) | UserQuery::Info { .. } | UserQuery::Lookup { .. } => {
                    Vec::new()
                }
            },
            Self::Resolve { .. } | Self::Dm { .. } => Vec::new(),
        }
    }

    pub(super) fn prepare(
        self,
        store: &Store,
        selected: Option<&str>,
        json: bool,
    ) -> Result<(ProfileView, app::Request)> {
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
                let page = paging.resolve(store.settings());
                match kind {
                    ResolveKind::User => Request::Resolve { name, page },
                    ResolveKind::Channel => {
                        let request = conversations::Request::Resolve { name, page };
                        return Ok((profile, app::Request::Conversations(request)));
                    }
                }
            }
            Self::Dm { user, options } => Request::Dm {
                user,
                approval: options.prepare(selected.is_some(), json)?,
            },
        };
        Ok((profile, app::Request::Users(request)))
    }
}

#[cfg(test)]
mod tests;
