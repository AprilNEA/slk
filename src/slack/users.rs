use super::{CursorPage, Page, Slack, pagination};
use crate::auth::Credentials;
use crate::error::{Error, Result};
use crate::messages::parse_channel;
use crate::users::{OpenedDm, User};
use serde::Deserialize;
use slack_morphism::api::SlackApiConversationsOpenRequest;
use slack_morphism::prelude::ValueStruct;
use slack_morphism::{
    SlackChannelId, SlackClientHttpConnector, SlackResponseMetadata, SlackUserId,
};

#[derive(Deserialize)]
struct UsersResponse {
    members: Vec<User>,
    response_metadata: Option<SlackResponseMetadata>,
    has_more: Option<bool>,
}

#[derive(Deserialize)]
struct UserResponse {
    user: User,
}

#[derive(Deserialize)]
struct OpenResponse {
    channel: DirectConversation,
    already_open: Option<bool>,
    no_op: Option<bool>,
}

#[derive(Deserialize)]
struct DirectConversation {
    id: SlackChannelId,
    user: SlackUserId,
    is_im: bool,
}

impl<C: SlackClientHttpConnector + Send + Sync> Slack<C> {
    pub(crate) async fn users(
        &self,
        credentials: &Credentials,
        page: &CursorPage,
    ) -> Result<Page<User>> {
        pagination::collect(page, |page| async move {
            let response: UsersResponse = self
                .get(
                    credentials,
                    "users.list",
                    vec![
                        ("limit", Some(&page.limit.to_string())),
                        ("cursor", page.cursor.as_deref()),
                    ],
                )
                .await?;
            Ok(Page::cursor(
                response.members,
                response.response_metadata,
                response.has_more,
            ))
        })
        .await
    }

    pub(crate) async fn user(
        &self,
        credentials: &Credentials,
        user_id: &SlackUserId,
    ) -> Result<User> {
        let response: UserResponse = self
            .get(
                credentials,
                "users.info",
                vec![("user", Some(user_id.value()))],
            )
            .await?;
        if response.user.user_id != *user_id {
            return Err(Error::Response);
        }
        Ok(response.user)
    }

    pub(crate) async fn user_by_email(
        &self,
        credentials: &Credentials,
        email: &str,
    ) -> Result<User> {
        let response: UserResponse = self
            .get(
                credentials,
                "users.lookupByEmail",
                vec![("email", Some(email))],
            )
            .await?;
        Ok(response.user)
    }

    pub(crate) async fn open_dm(
        &self,
        credentials: &Credentials,
        user_id: &SlackUserId,
    ) -> Result<OpenedDm> {
        let request = SlackApiConversationsOpenRequest::new()
            .with_users(vec![user_id.clone()])
            .with_return_im(true);
        let response: OpenResponse = self
            .write(credentials, "conversations.open", &request)
            .await?;
        if response.channel.user != *user_id
            || !response.channel.is_im
            || !response.channel.id.value().starts_with('D')
            || parse_channel(response.channel.id.value()).is_err()
        {
            return Err(Error::WriteOutcomeUnknown { slack_error: None });
        }
        Ok(OpenedDm {
            channel_id: response.channel.id,
            user_id: response.channel.user,
            already_open: response.already_open,
            no_op: response.no_op,
        })
    }
}
