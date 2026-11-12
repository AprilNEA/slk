use super::{
    Effect, FilePath, Paging,
    messages::Target,
    write::{WriteInput, WriteOptions},
};
use crate::app::{self, messages::Destination, scheduled::Request};
use crate::auth::{ProfileView, Store};
use crate::error::Result;
use crate::ids::{parse_channel, parse_scheduled_message};
use crate::model::scheduled::validate_time;
use chrono::{DateTime, FixedOffset, Utc};
use clap::{Args, Subcommand};
use slack_morphism::SlackChannelId;

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
    input: WriteInput,
}

impl ScheduledCommand {
    pub(super) fn effect(&self) -> Effect {
        match self {
            Self::List { .. } => Effect::Read,
            Self::Create { input, .. } | Self::Reply { input, .. } => input.input.options.effect(),
            Self::Cancel { options, .. } => options.effect(),
        }
    }

    pub(super) fn file_paths(&mut self) -> Vec<FilePath<'_>> {
        match self {
            Self::Create { input, .. } | Self::Reply { input, .. } => input.input.file_paths(),
            Self::List { .. } | Self::Cancel { .. } => Vec::new(),
        }
    }

    pub(super) fn prepare(
        self,
        store: &Store,
        selected: Option<&str>,
        json: bool,
    ) -> Result<(ProfileView, app::Request)> {
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
        Ok((profile, app::Request::Scheduled(request)))
    }
}

#[cfg(test)]
mod tests;
