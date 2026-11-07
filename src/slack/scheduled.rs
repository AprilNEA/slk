use super::{CursorPage, Page, Slack, pagination};
use crate::auth::Credentials;
use crate::error::{Error, Result};
use crate::messages::Draft;
use crate::scheduled::{Receipt, Scheduled, parse_id};
use serde::Deserialize;
use serde_json::json;
use slack_morphism::{
    SlackApiToken, SlackChannelId, SlackClientHttpConnector, SlackResponseMetadata,
};

#[derive(Deserialize)]
struct ListResponse {
    scheduled_messages: Vec<Scheduled>,
    response_metadata: Option<SlackResponseMetadata>,
}

impl<C: SlackClientHttpConnector + Send + Sync> Slack<C> {
    pub(crate) async fn schedule(
        &self,
        credentials: &Credentials,
        draft: &Draft,
        post_at: i64,
    ) -> Result<Receipt> {
        let mut body =
            serde_json::to_value(super::message_request(draft)).map_err(|_| Error::Response)?;
        body["post_at"] = json!(post_at);
        let receipt: Receipt = self
            .write(credentials, "chat.scheduleMessage", &body)
            .await?;
        if receipt.channel_id != draft.channel_id
            || receipt.post_at != post_at
            || parse_id(&receipt.scheduled_message_id).is_err()
        {
            return Err(Error::WriteOutcomeUnknown { slack_error: None });
        }
        Ok(receipt)
    }

    pub(crate) async fn scheduled(
        &self,
        credentials: &Credentials,
        channel: &SlackChannelId,
        page: &CursorPage,
    ) -> Result<Page<Scheduled>> {
        pagination::collect(page, |page| async move {
            let token = SlackApiToken::from(credentials);
            let session = self.client.open_session(&token);
            let body = json!({"channel": channel, "limit": page.limit, "cursor": page.cursor});
            let response: ListResponse = self
                .response(session.http_session_api.http_post(
                    "chat.scheduledMessages.list",
                    &body,
                    None,
                ))
                .await?;
            if response.scheduled_messages.iter().any(|message| {
                message.receipt.channel_id != *channel
                    || parse_id(&message.receipt.scheduled_message_id).is_err()
            }) {
                return Err(Error::Response);
            }
            Ok(Page::cursor(
                response.scheduled_messages,
                response.response_metadata,
                None,
            ))
        })
        .await
    }

    pub(crate) async fn cancel_schedule(
        &self,
        credentials: &Credentials,
        receipt: &Receipt,
    ) -> Result<()> {
        let _: serde_json::Value = self.write(credentials, "chat.deleteScheduledMessage", &json!({
            "channel": receipt.channel_id, "scheduled_message_id": receipt.scheduled_message_id,
        })).await?;
        Ok(())
    }
}
