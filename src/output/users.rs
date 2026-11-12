use super::{identity_text, page_text, resolution_text, terminal_text};
use crate::auth::ProfileView;
use crate::model::pagination::{Page, Resolution};
use crate::model::users::{OpenedDm, User};
use serde::Serialize;
use slack_morphism::prelude::ValueStruct;

#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum Output {
    Users(Page<User>),
    User(User),
    ResolvedUsers(Resolution<User>),
    DmPreview {
        dry_run: bool,
        #[serde(flatten)]
        profile: ProfileView,
        target: User,
    },
    DmOpened(OpenedDm),
}

impl Output {
    pub(super) fn text(&self) -> String {
        match self {
            Self::Users(page) => {
                page_text(page.data.iter().map(user_text).collect(), &page.pagination)
            }
            Self::User(user) => user_text(user),
            Self::ResolvedUsers(result) => resolution_text(result, user_text),
            Self::DmPreview {
                profile, target, ..
            } => format!(
                "Dry run; no conversation was opened or created.\n{}",
                dm_preview_text(profile, target)
            ),
            Self::DmOpened(opened) => format!(
                "Opened DM\t{}\t{}\talready_open:{}\tno_op:{}\nNo message was sent.",
                terminal_text(opened.channel_id.value()),
                terminal_text(opened.user_id.value()),
                opened
                    .already_open
                    .map_or_else(|| "unknown".to_owned(), |value| value.to_string()),
                opened
                    .no_op
                    .map_or_else(|| "unknown".to_owned(), |value| value.to_string()),
            ),
        }
    }
}

pub(super) fn user_text(user: &User) -> String {
    let profile = user.profile.as_ref();
    format!(
        "{}\t{}\t{}\t{}\t{}{}{}",
        terminal_text(user.user_id.value()),
        terminal_text(user.name.as_deref().unwrap_or("-")),
        terminal_text(
            profile
                .and_then(|value| value.display_name.as_deref())
                .unwrap_or("-")
        ),
        terminal_text(
            user.real_name
                .as_deref()
                .or(profile.and_then(|value| value.real_name.as_deref()))
                .unwrap_or("-")
        ),
        terminal_text(
            profile
                .and_then(|value| value.email.as_deref())
                .unwrap_or("-")
        ),
        if user.deleted { " (deactivated)" } else { "" },
        if user.is_bot || user.is_app_user {
            " (app)"
        } else {
            ""
        },
    )
}

pub(crate) fn dm_preview_text(profile: &ProfileView, user: &User) -> String {
    format!(
        "{}\nAction: open or create a single-person DM\nTarget: {}\nNo message will be sent.",
        identity_text(profile),
        user_text(user),
    )
}
