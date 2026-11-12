use super::{
    Paging,
    messages::Target,
    write::{WriteInput, WriteOptions},
};
use crate::app::{Approval, Connection, Prompt, messages::Destination};
use crate::auth::{ProfileView, Store};
use crate::error::{Error, Result};
use crate::ids::{parse_channel, parse_scheduled_message};
use crate::model::messages::Content;
use crate::output::{self, Output};
use crate::scheduled::validate_time;
use crate::slack::CursorPage;
use chrono::{DateTime, FixedOffset, Utc};
use clap::{Args, Subcommand};
use slack_morphism::{SlackChannelId, SlackClientHttpConnector};

#[derive(Subcommand)]
pub(super) enum ScheduledCommand {
    /// Schedule one top-level message on Slack, without a local background process.
    Create {
        #[arg(value_parser = parse_channel)]
        conversation: SlackChannelId,
        #[command(flatten)]
        input: TimedInput,
    },
    /// Schedule one reply to a verified thread root, without broadcasting.
    Reply {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        input: TimedInput,
    },
    /// List API schedules for this token, not Slack's UI; one page by default.
    List {
        #[arg(value_parser = parse_channel)]
        conversation: SlackChannelId,
        #[command(flatten)]
        paging: Paging,
    },
    /// Inspect one exact schedule in the selected pages, then confirm cancellation.
    Cancel {
        #[arg(value_parser = parse_channel)]
        conversation: SlackChannelId,
        #[arg(value_parser = parse_scheduled_message)]
        scheduled_message_id: String,
        #[command(flatten)]
        paging: Paging,
        #[command(flatten)]
        options: WriteOptions,
    },
}

#[derive(Args)]
pub(super) struct TimedInput {
    /// RFC3339 date with a timezone and whole seconds, at most 120 days in the future.
    #[arg(long, value_name = "RFC3339")]
    at: DateTime<FixedOffset>,
    #[command(flatten)]
    pub(super) input: WriteInput,
}

enum Request {
    Create {
        destination: Destination,
        at: DateTime<FixedOffset>,
        content: Content,
        approval: Approval,
    },
    List {
        channel: SlackChannelId,
        page: CursorPage,
    },
    Cancel {
        channel: SlackChannelId,
        id: String,
        page: CursorPage,
        approval: Approval,
    },
}

pub(super) struct Prepared {
    request: Request,
}

impl ScheduledCommand {
    pub(super) fn prepare(
        self,
        store: &Store,
        selected: Option<&str>,
        json: bool,
    ) -> Result<(ProfileView, Prepared)> {
        let profile = store.select(selected)?;
        let request = match self {
            Self::Create {
                conversation,
                input,
            } => {
                validate_time(input.at, Utc::now())?;
                let (content, approval) = input.input.prepare(selected.is_some(), json)?;
                Request::Create {
                    destination: Destination::Conversation(conversation),
                    at: input.at,
                    content,
                    approval,
                }
            }
            Self::Reply { target, input } => {
                let target = target.reference(&profile)?;
                validate_time(input.at, Utc::now())?;
                let (content, approval) = input.input.prepare(selected.is_some(), json)?;
                Request::Create {
                    destination: Destination::Thread(target),
                    at: input.at,
                    content,
                    approval,
                }
            }
            Self::List {
                conversation,
                paging,
            } => Request::List {
                channel: conversation,
                page: paging.resolve(store.settings()),
            },
            Self::Cancel {
                conversation,
                scheduled_message_id,
                paging,
                options,
            } => Request::Cancel {
                channel: conversation,
                id: scheduled_message_id,
                page: paging.resolve(store.settings()),
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
            Request::Create {
                destination,
                at,
                content,
                approval,
            } => {
                let draft = destination.draft(&slack, credentials, content).await?;
                let post_at = validate_time(at, Utc::now())?;
                if !approval.confirm(prompt, "Schedule this message?", || {
                    output::schedule_preview_text(&profile, &draft, post_at)
                })? {
                    return Ok(Output::SchedulePreview {
                        dry_run: true,
                        profile,
                        draft,
                        post_at,
                    });
                }
                validate_time(at, Utc::now())?;
                slack
                    .schedule(credentials, &draft, post_at)
                    .await
                    .map(Output::Scheduled)
            }
            Request::List { channel, page } => slack
                .scheduled(credentials, &channel, &page)
                .await
                .map(Output::Schedules),
            Request::Cancel {
                channel,
                id,
                page,
                approval,
            } => {
                let schedules = slack.scheduled(credentials, &channel, &page).await?;
                let mut matches = schedules
                    .data
                    .into_iter()
                    .filter(|item| item.receipt.scheduled_message_id == id);
                let scheduled = matches.next().ok_or(Error::ScheduledNotFound)?;
                if matches.next().is_some() {
                    return Err(Error::Response);
                }
                if !approval.confirm(prompt, "Cancel this scheduled message?", || {
                    output::cancel_preview_text(&profile, &scheduled)
                })? {
                    return Ok(Output::ScheduleCancellationPreview {
                        dry_run: true,
                        profile,
                        scheduled,
                    });
                }
                slack
                    .cancel_schedule(credentials, &scheduled.receipt)
                    .await?;
                Ok(Output::ScheduleCancelled {
                    cancelled: scheduled.receipt,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests;
