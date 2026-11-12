use super::write::{SendInput, WriteInput, WriteOptions};
use super::{Paging, Traversal};
use crate::app::{Approval, Connection, Prompt};
use crate::auth::{Credentials, ProfileView, Store};
use crate::error::{Error, Result};
use crate::ids::{parse_channel, parse_timestamp};
use crate::messages::{Change, Content, Draft, MessageLink, parse_reaction};
use crate::output::{self, Output};
use crate::slack::{
    CursorPage, MAX_SEARCH_PAGE, MAX_SEARCH_SIZE, SearchPage, Slack, ThreadRequest,
};
use clap::{Args, Subcommand};
use slack_morphism::{SlackChannelId, SlackClientHttpConnector, SlackReactionName, SlackTs};

#[derive(Subcommand)]
pub(super) enum MessageCommand {
    /// List visible conversations without joining them; one page by default.
    Channels(Paging),
    /// Search with RTS (user) or experimental legacy search (browser).
    Search {
        /// A native Slack query; quote queries containing spaces.
        query: String,
        /// With `search messages QUERY`, use QUERY as the Slack query.
        #[arg(value_name = "SLACKCLI_QUERY")]
        compatibility_query: Option<String>,
        #[command(flatten)]
        selector: super::compat::Selector,
        /// Maximum results per page; capped at the RTS maximum.
        #[arg(long, value_parser = clap::value_parser!(u16).range(1..=i64::from(MAX_SEARCH_SIZE)))]
        limit: Option<u16>,
        /// Continue official user search with this cursor.
        #[arg(long, conflicts_with = "page")]
        cursor: Option<String>,
        /// Fetch a browser search page, starting at 1.
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..=i64::from(MAX_SEARCH_PAGE)))]
        page: Option<u32>,
        #[command(flatten)]
        traversal: Traversal,
    },
    /// Read conversation history, newest first; one page by default.
    Read {
        #[arg(value_parser = parse_channel)]
        conversation: SlackChannelId,
        #[command(flatten)]
        paging: Paging,
    },
    /// Read the exact message in a permalink, not its thread root.
    Get {
        #[command(flatten)]
        target: Target,
    },
    /// Read a verified thread, oldest first; subsequent pages may omit the root.
    Thread {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        paging: Paging,
    },
    /// Send to an existing conversation; --split creates ordered top-level messages.
    Send {
        #[arg(value_parser = parse_channel)]
        conversation: SlackChannelId,
        #[command(flatten)]
        input: SendInput,
    },
    /// Reply to a verified thread root, never broadcast to the channel.
    Reply {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        input: SendInput,
    },
    /// Edit your exact message; replacing blocks requires --blocks-file.
    Edit {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        input: WriteInput,
    },
    /// Delete your own text-only message at the URL's exact timestamp.
    Delete {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        options: WriteOptions,
    },
    /// Add an emoji reaction to the exact linked message, not its thread root.
    React {
        #[command(flatten)]
        target: Target,
        /// Emoji name without surrounding colons, such as thumbsup::skin-tone-3.
        #[arg(value_parser = parse_reaction)]
        name: SlackReactionName,
        #[command(flatten)]
        options: WriteOptions,
    },
    /// Remove your emoji reaction from the exact linked message.
    Unreact {
        #[command(flatten)]
        target: Target,
        #[arg(value_parser = parse_reaction)]
        name: SlackReactionName,
        #[command(flatten)]
        options: WriteOptions,
    },
    /// List channel pins, including opaque file and future item kinds; not personal Later items.
    Pins {
        #[arg(value_parser = parse_channel)]
        conversation: SlackChannelId,
    },
    /// Pin the exact linked message for everyone in the conversation.
    Pin {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        options: WriteOptions,
    },
    /// Unpin the exact linked message without deleting it.
    Unpin {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        options: WriteOptions,
    },
}

#[derive(Args)]
pub(super) struct Target {
    /// HTTPS message permalink in the selected workspace.
    pub(super) message_url: MessageLink,
    /// Explicit root timestamp when the URL does not identify the root.
    #[arg(long, value_parser = parse_timestamp)]
    pub(super) thread: Option<SlackTs>,
}

impl Target {
    pub(super) fn validate(&self, profile: &ProfileView) -> Result<()> {
        self.message_url
            .check_workspace(&profile.identity.workspace_url)?;
        self.message_url.root_hint(self.thread.as_ref())?;
        Ok(())
    }
}

pub(super) enum Destination {
    Conversation(SlackChannelId),
    Thread(Target),
}

impl Destination {
    pub(super) async fn draft<C: SlackClientHttpConnector + Send + Sync>(
        self,
        slack: &Slack<C>,
        credentials: &Credentials,
        content: Content,
    ) -> Result<Draft> {
        let (channel_id, thread_ts) = match self {
            Self::Conversation(channel) => (channel, None),
            Self::Thread(target) => {
                let root = slack
                    .resolve_thread(credentials, &target.message_url, target.thread.as_ref())
                    .await?;
                (target.message_url.channel_id, Some(root))
            }
        };
        Ok(Draft {
            channel_id,
            thread_ts,
            content,
        })
    }
}

enum Request {
    Channels(CursorPage),
    Pins(SlackChannelId),
    Get(Target),
    Search {
        query: String,
        page: SearchPage,
    },
    Read {
        channel: SlackChannelId,
        page: CursorPage,
    },
    Thread {
        target: Target,
        page: CursorPage,
    },
    Write {
        destination: Destination,
        content: Vec<Content>,
        approval: Approval,
    },
    Modify {
        target: Target,
        change: Change,
        approval: Approval,
    },
}

pub(super) struct Prepared {
    request: Request,
}

impl MessageCommand {
    pub(super) fn prepare(
        self,
        store: &Store,
        selected: Option<&str>,
        json: bool,
    ) -> Result<(ProfileView, Prepared)> {
        let selection = match &self {
            Self::Search { selector, .. } => selector.resolve(store, selected)?,
            _ => selected.map(str::to_owned),
        };
        let selected = selection.as_deref();
        let profile = store.select(selected)?;
        let settings = store.settings();
        let request = match self {
            Self::Channels(paging) => Request::Channels(paging.resolve(settings)),
            Self::Search {
                query,
                compatibility_query,
                selector: _,
                limit,
                cursor,
                page,
                traversal,
            } => {
                let query = match compatibility_query {
                    Some(value) if query == "messages" => value,
                    Some(_) => {
                        return Err(Error::Arguments(
                            "Quote the entire search query, or use search messages QUERY.",
                        ));
                    }
                    None => query,
                };
                if query.trim().is_empty() {
                    return Err(Error::Arguments("The search query must not be empty."));
                }
                let page = SearchPage {
                    limit: limit.unwrap_or(settings.page_size.get().min(MAX_SEARCH_SIZE)),
                    cursor,
                    page,
                    max_pages: traversal.budget(settings),
                };
                page.validate(profile.kind, profile.identity.workspace_url.realm())?;
                Request::Search { query, page }
            }
            Self::Read {
                conversation,
                paging,
            } => Request::Read {
                channel: conversation,
                page: paging.resolve(settings),
            },
            Self::Get { target } => {
                target.validate(&profile)?;
                Request::Get(target)
            }
            Self::Thread { target, paging } => {
                target.validate(&profile)?;
                Request::Thread {
                    target,
                    page: paging.resolve(settings),
                }
            }
            Self::Send {
                conversation,
                input,
            } => {
                let (content, approval) = input.prepare(settings, selected.is_some(), json)?;
                Request::Write {
                    destination: Destination::Conversation(conversation),
                    content,
                    approval,
                }
            }
            Self::Reply { target, input } => {
                target.validate(&profile)?;
                let (content, approval) = input.prepare(settings, selected.is_some(), json)?;
                Request::Write {
                    destination: Destination::Thread(target),
                    content,
                    approval,
                }
            }
            Self::Edit { target, input } => {
                target.validate(&profile)?;
                let (content, approval) = input.prepare(selected.is_some(), json)?;
                Request::Modify {
                    target,
                    change: Change::Edit { content },
                    approval,
                }
            }
            Self::Delete { target, options } => {
                target.validate(&profile)?;
                Request::Modify {
                    target,
                    change: Change::Delete,
                    approval: options.prepare(selected.is_some(), json)?,
                }
            }
            Self::React {
                target,
                name,
                options,
            } => {
                target.validate(&profile)?;
                Request::Modify {
                    target,
                    change: Change::React { name },
                    approval: options.prepare(selected.is_some(), json)?,
                }
            }
            Self::Unreact {
                target,
                name,
                options,
            } => {
                target.validate(&profile)?;
                Request::Modify {
                    target,
                    change: Change::Unreact { name },
                    approval: options.prepare(selected.is_some(), json)?,
                }
            }
            Self::Pins { conversation } => Request::Pins(conversation),
            Self::Pin { target, options } => {
                target.validate(&profile)?;
                Request::Modify {
                    target,
                    change: Change::Pin,
                    approval: options.prepare(selected.is_some(), json)?,
                }
            }
            Self::Unpin { target, options } => {
                target.validate(&profile)?;
                Request::Modify {
                    target,
                    change: Change::Unpin,
                    approval: options.prepare(selected.is_some(), json)?,
                }
            }
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
            Request::Channels(page) => slack
                .channels(credentials, &page)
                .await
                .map(Output::Channels),
            Request::Pins(channel_id) => {
                let data = slack.pins(credentials, &channel_id).await?;
                Ok(Output::Pins { channel_id, data })
            }
            Request::Search { query, page } => slack
                .search(credentials, &query, &page)
                .await
                .map(Output::Messages),
            Request::Read { channel, page } => slack
                .history(credentials, &channel, &page)
                .await
                .map(Output::Messages),
            Request::Get(target) => slack
                .message(credentials, &target.message_url, target.thread.as_ref())
                .await
                .map(|target| Output::Message(target.message)),
            Request::Thread { target, page } => {
                let root = slack
                    .resolve_thread(credentials, &target.message_url, target.thread.as_ref())
                    .await?;
                slack
                    .replies(
                        credentials,
                        &ThreadRequest {
                            channel_id: &target.message_url.channel_id,
                            thread_ts: &root,
                            page: &page,
                        },
                    )
                    .await
                    .map(Output::Messages)
            }
            Request::Write {
                destination,
                content,
                approval,
            } => {
                let mut content = content.into_iter();
                let first = content
                    .next()
                    .expect("validated input contains at least one chunk");
                let first = destination.draft(&slack, credentials, first).await?;
                let mut drafts = vec![first];
                for content in content {
                    drafts.push(Draft {
                        channel_id: drafts[0].channel_id.clone(),
                        thread_ts: drafts[0].thread_ts.clone(),
                        content,
                    });
                }
                if !approval.confirm(prompt, "Send these messages in order?", || {
                    output::messages::batch_preview_text(&profile, &drafts)
                })? {
                    return Ok(if drafts.len() == 1 {
                        Output::Preview {
                            dry_run: true,
                            profile,
                            draft: drafts.pop().expect("one preview remains"),
                        }
                    } else {
                        Output::BatchPreview {
                            dry_run: true,
                            profile,
                            drafts,
                        }
                    });
                }
                let total_chunks = drafts.len();
                let mut sent = Vec::new();
                for draft in drafts {
                    match slack.send(credentials, &draft).await {
                        Ok(receipt) => sent.push(receipt),
                        Err(source) if total_chunks == 1 => return Err(source),
                        Err(source) => {
                            return Err(Error::SplitWrite {
                                failed_chunk: sent.len() + 1,
                                total_chunks,
                                sent,
                                source: Box::new(source),
                            });
                        }
                    }
                }
                Ok(if total_chunks == 1 {
                    Output::Sent(sent.pop().expect("the single send succeeded"))
                } else {
                    Output::SentBatch { data: sent }
                })
            }
            Request::Modify {
                target,
                change,
                approval,
            } => {
                let inspected = slack
                    .message(credentials, &target.message_url, target.thread.as_ref())
                    .await?;
                change.validate(&inspected, &profile.identity)?;
                let message = inspected.message;
                if !approval.confirm(prompt, "Apply this change?", || {
                    output::messages::change_preview_text(&profile, &message, &change)
                })? {
                    return Ok(Output::ChangePreview {
                        dry_run: true,
                        profile,
                        message,
                        change,
                    });
                }
                slack
                    .change(credentials, message, change)
                    .await
                    .map(Output::Changed)
            }
        }
    }
}

#[cfg(test)]
mod tests;
