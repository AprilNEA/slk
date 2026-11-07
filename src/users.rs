use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use slack_morphism::{SlackChannelId, SlackTeamId, SlackUserId};

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct User {
    #[serde(rename(deserialize = "id"))]
    pub user_id: SlackUserId,
    pub team_id: Option<SlackTeamId>,
    pub name: Option<String>,
    pub real_name: Option<String>,
    pub profile: Option<UserProfile>,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub is_bot: bool,
    #[serde(default)]
    pub is_app_user: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct UserProfile {
    pub display_name: Option<String>,
    pub real_name: Option<String>,
    pub email: Option<String>,
}

impl User {
    pub(crate) fn matches_name(&self, name: &str) -> bool {
        [self.name.as_deref(), self.real_name.as_deref()]
            .into_iter()
            .chain(self.profile.as_ref().into_iter().flat_map(|profile| {
                [
                    profile.display_name.as_deref(),
                    profile.real_name.as_deref(),
                ]
            }))
            .flatten()
            .any(|candidate| candidate.eq_ignore_ascii_case(name))
    }
}

pub(crate) fn parse_user(value: &str) -> Result<SlackUserId> {
    if value.len() < 2
        || !value.starts_with(['U', 'W'])
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err(Error::Arguments(
            "Use a user ID starting with U or W, not a name, email, or conversation ID.",
        ));
    }
    Ok(SlackUserId(value.to_owned()))
}

#[derive(Debug, Serialize)]
pub(crate) struct OpenedDm {
    pub channel_id: SlackChannelId,
    pub user_id: SlackUserId,
    pub already_open: Option<bool>,
    pub no_op: Option<bool>,
}
