use crate::error::{Error, Result};
use slack_morphism::{
    SlackCanvasId, SlackCanvasSectionId, SlackChannelId, SlackFileId, SlackTs, SlackUserId,
};

/// Reports whether `value` is an uppercase alphanumeric Slack ID starting with one of `prefixes`.
pub(crate) fn is_id(value: &str, prefixes: &[char]) -> bool {
    value.len() >= 2
        && value.starts_with(prefixes)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

pub(crate) fn parse_channel(value: &str) -> Result<SlackChannelId> {
    if !is_id(value, &['C', 'G', 'D']) {
        return Err(Error::Arguments(
            "Use a conversation ID starting with C, G, or D, not a name or user ID.",
        ));
    }
    Ok(SlackChannelId(value.to_owned()))
}

pub(crate) fn parse_timestamp(value: &str) -> Result<SlackTs> {
    let valid = value.split_once('.').is_some_and(|(seconds, fraction)| {
        !seconds.is_empty()
            && fraction.len() == 6
            && seconds
                .bytes()
                .chain(fraction.bytes())
                .all(|byte| byte.is_ascii_digit())
    });
    if !valid {
        return Err(Error::Arguments(
            "Use a Slack timestamp in seconds.microseconds format with six fractional digits.",
        ));
    }
    Ok(SlackTs(value.to_owned()))
}

pub(crate) fn parse_user(value: &str) -> Result<SlackUserId> {
    if !is_id(value, &['U', 'W']) {
        return Err(Error::Arguments(
            "Use a user ID starting with U or W, not a name, email, or conversation ID.",
        ));
    }
    Ok(SlackUserId(value.to_owned()))
}

pub(crate) fn parse_file(value: &str) -> Result<SlackFileId> {
    if !is_id(value, &['F']) {
        return Err(Error::Arguments(
            "Use an explicit Slack file ID starting with F, not a URL.",
        ));
    }
    Ok(SlackFileId(value.to_owned()))
}

pub(crate) fn parse_scheduled_message(value: &str) -> Result<String> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err(Error::Arguments(
            "Use the explicit scheduled_message_id returned by Slack.",
        ));
    }
    Ok(value.to_owned())
}

pub(crate) fn parse_canvas(value: &str) -> Result<SlackCanvasId> {
    Ok(SlackCanvasId(parse_file(value)?.0))
}

pub(crate) fn parse_section(value: &str) -> Result<SlackCanvasSectionId> {
    if value.is_empty()
        || value
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control())
    {
        return Err(Error::Arguments(
            "Use a nonempty section ID returned by canvas sections, without whitespace or controls.",
        ));
    }
    Ok(SlackCanvasSectionId(value.to_owned()))
}
