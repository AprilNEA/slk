use super::messages::preview_text;
use super::{identity_text, page_text, terminal_text};
use crate::auth::ProfileView;
use crate::model::messages::Draft;
use crate::model::pagination::Page;
use crate::model::scheduled::{Receipt, Scheduled};
use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum Output {
    Scheduled(Receipt),
    Schedules(Page<Scheduled>),
    SchedulePreview {
        dry_run: bool,
        #[serde(flatten)]
        profile: ProfileView,
        #[serde(flatten)]
        draft: Draft,
        post_at: i64,
    },
    ScheduleCancellationPreview {
        dry_run: bool,
        #[serde(flatten)]
        profile: ProfileView,
        scheduled: Scheduled,
    },
    ScheduleCancelled {
        cancelled: Receipt,
    },
}

impl Output {
    pub(super) fn text(&self) -> String {
        match self {
            Self::Scheduled(receipt) => {
                format!("Scheduled on Slack\t{}", receipt_text(receipt))
            }
            Self::Schedules(page) => page_text(
                page.data.iter().map(scheduled_text).collect(),
                &page.pagination,
            ),
            Self::SchedulePreview {
                profile,
                draft,
                post_at,
                ..
            } => format!(
                "Dry run; no message was scheduled.\n{}",
                schedule_preview_text(profile, draft, *post_at)
            ),
            Self::ScheduleCancellationPreview {
                profile, scheduled, ..
            } => format!(
                "Dry run; no schedule was cancelled.\n{}",
                cancel_preview_text(profile, scheduled)
            ),
            Self::ScheduleCancelled { cancelled } => {
                format!("Cancelled schedule\t{}", receipt_text(cancelled))
            }
        }
    }
}

fn receipt_text(receipt: &Receipt) -> String {
    format!(
        "{}\t{}\t{}",
        terminal_text(&receipt.scheduled_message_id),
        terminal_text(&receipt.channel_id.0),
        time_text(receipt.post_at)
    )
}

fn scheduled_text(scheduled: &Scheduled) -> String {
    format!(
        "{}\n{}",
        receipt_text(&scheduled.receipt),
        terminal_text(scheduled.text.as_deref().unwrap_or("[text unavailable]"))
    )
}

pub(crate) fn schedule_preview_text(profile: &ProfileView, draft: &Draft, post_at: i64) -> String {
    format!(
        "Action: schedule on Slack\nPost at: {}\n{}",
        time_text(post_at),
        preview_text(profile, draft)
    )
}

pub(crate) fn cancel_preview_text(profile: &ProfileView, scheduled: &Scheduled) -> String {
    format!(
        "{}\nAction: cancel this scheduled message\n{}",
        identity_text(profile),
        scheduled_text(scheduled)
    )
}

fn time_text(seconds: i64) -> String {
    match DateTime::<Utc>::from_timestamp(seconds, 0) {
        Some(time) => format!("{} ({seconds})", time.to_rfc3339()),
        None => seconds.to_string(),
    }
}
