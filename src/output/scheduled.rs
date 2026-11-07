use super::messages::preview_text;
use super::{identity_text, terminal_text};
use crate::auth::ProfileView;
use crate::messages::Draft;
use crate::scheduled::{Receipt, Scheduled};
use chrono::{DateTime, Utc};

pub(super) fn receipt_text(receipt: &Receipt) -> String {
    format!(
        "{}\t{}\t{}",
        terminal_text(&receipt.scheduled_message_id),
        terminal_text(&receipt.channel_id.0),
        time_text(receipt.post_at)
    )
}

pub(super) fn scheduled_text(scheduled: &Scheduled) -> String {
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
