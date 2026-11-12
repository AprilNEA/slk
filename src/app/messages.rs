use super::{Approval, Connection, Prompt};
use crate::auth::Credentials;
use crate::error::{Error, Result};
use crate::model::messages::{Change, Content, Draft, MessageReference, SearchPage};
use crate::model::pagination::CursorPage;
use crate::output::messages::{Output, batch_preview_text, change_preview_text};
use crate::slack::Slack;
use crate::slack::messages::ThreadRequest;
use slack_morphism::{SlackChannelId, SlackClientHttpConnector};

/// Where a new message goes: a conversation, or the thread that a message reference identifies.
pub(crate) enum Destination {
    Conversation(SlackChannelId),
    Thread(MessageReference),
}

impl Destination {
    /// Addresses `content` to this destination after Slack verifies the thread root.
    pub(crate) async fn draft<C: SlackClientHttpConnector + Send + Sync>(
        self,
        slack: &Slack<C>,
        credentials: &Credentials,
        content: Content,
    ) -> Result<Draft> {
        let (channel_id, thread_ts) = match self {
            Self::Conversation(channel) => (channel, None),
            Self::Thread(target) => {
                let root = slack
                    .resolve_thread(credentials, &target.link, target.thread.as_ref())
                    .await?;
                (target.link.channel_id, Some(root))
            }
        };
        Ok(Draft {
            channel_id,
            thread_ts,
            content,
        })
    }
}

/// The order of the messages in a history result. Slack returns history newest first.
pub(crate) enum Order {
    NewestFirst,
    OldestFirst,
}

pub(crate) enum Request {
    Pins(SlackChannelId),
    Get(MessageReference),
    Search {
        query: String,
        page: SearchPage,
    },
    Read {
        channel: SlackChannelId,
        page: CursorPage,
        order: Order,
    },
    Thread {
        target: MessageReference,
        page: CursorPage,
    },
    Write {
        destination: Destination,
        content: Vec<Content>,
        approval: Approval,
    },
    Modify {
        target: MessageReference,
        change: Change,
        approval: Approval,
    },
}

impl Request {
    pub(crate) async fn execute<C: SlackClientHttpConnector + Send + Sync>(
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
        match self {
            Self::Pins(channel_id) => {
                let data = slack.pins(credentials, &channel_id).await?;
                Ok(Output::Pins { channel_id, data })
            }
            Self::Search { query, page } => slack
                .search(credentials, &query, &page)
                .await
                .map(Output::Messages),
            Self::Read {
                channel,
                page,
                order,
            } => {
                let mut messages = slack.history(credentials, &channel, &page).await?;
                if matches!(order, Order::OldestFirst) {
                    messages.data.reverse();
                }
                Ok(Output::Messages(messages))
            }
            Self::Get(target) => slack
                .message(credentials, &target.link, target.thread.as_ref())
                .await
                .map(|target| Output::Message(target.message)),
            Self::Thread { target, page } => {
                let root = slack
                    .resolve_thread(credentials, &target.link, target.thread.as_ref())
                    .await?;
                slack
                    .replies(
                        credentials,
                        &ThreadRequest {
                            channel_id: &target.link.channel_id,
                            thread_ts: &root,
                            page: &page,
                        },
                    )
                    .await
                    .map(Output::Messages)
            }
            Self::Write {
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
                    batch_preview_text(&profile, &drafts)
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
            Self::Modify {
                target,
                change,
                approval,
            } => {
                let inspected = slack
                    .message(credentials, &target.link, target.thread.as_ref())
                    .await?;
                change.validate(&inspected, &profile.identity)?;
                let message = inspected.message;
                if !approval.confirm(prompt, "Apply this change?", || {
                    change_preview_text(&profile, &message, &change)
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
