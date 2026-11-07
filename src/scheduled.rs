use crate::error::{Error, Result};
use chrono::{DateTime, FixedOffset, Utc};
use serde::{Deserialize, Serialize};
use serde_aux::field_attributes::{deserialize_number_from_string, deserialize_string_from_number};
use slack_morphism::SlackChannelId;

// Slack limits service-side schedules to 120 days, independently of client settings.
const MAX_DELAY_SECONDS: i64 = 120 * 24 * 60 * 60;

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct Receipt {
    #[serde(alias = "channel")]
    pub channel_id: SlackChannelId,
    #[serde(alias = "id", deserialize_with = "deserialize_string_from_number")]
    pub scheduled_message_id: String,
    #[serde(deserialize_with = "deserialize_number_from_string")]
    pub post_at: i64,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct Scheduled {
    #[serde(flatten)]
    pub receipt: Receipt,
    pub text: Option<String>,
}

pub(crate) fn validate_time(time: DateTime<FixedOffset>, now: DateTime<Utc>) -> Result<i64> {
    let seconds = time.timestamp() - now.timestamp();
    if seconds <= 0 || seconds > MAX_DELAY_SECONDS || time.timestamp_subsec_nanos() != 0 {
        return Err(Error::Arguments(
            "Use an RFC3339 time with an explicit timezone, whole seconds, and a future date within Slack's 120-day limit.",
        ));
    }
    Ok(time.timestamp())
}

pub(crate) fn parse_id(value: &str) -> Result<String> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err(Error::Arguments(
            "Use the explicit scheduled_message_id returned by Slack.",
        ));
    }
    Ok(value.to_owned())
}
