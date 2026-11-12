use super::messages::Destination;
use super::{Approval, Connection, Prompt};
use crate::error::{Error, Result};
use crate::model::messages::Content;
use crate::model::scheduled::validate_time;
use crate::output::scheduled::{Output, cancel_preview_text, schedule_preview_text};
use crate::slack::CursorPage;
use chrono::{DateTime, FixedOffset, Utc};
use slack_morphism::{SlackChannelId, SlackClientHttpConnector};

pub(crate) enum Request {
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
            Self::Create {
                destination,
                at,
                content,
                approval,
            } => {
                let draft = destination.draft(&slack, credentials, content).await?;
                let post_at = validate_time(at, Utc::now())?;
                if !approval.confirm(prompt, "Schedule this message?", || {
                    schedule_preview_text(&profile, &draft, post_at)
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
            Self::List { channel, page } => slack
                .scheduled(credentials, &channel, &page)
                .await
                .map(Output::Schedules),
            Self::Cancel {
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
                    cancel_preview_text(&profile, &scheduled)
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
