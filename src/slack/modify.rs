use super::Slack;
use crate::auth::Credentials;
use crate::error::{Error, Result};
use crate::messages::{Change, Changed, Message};
use serde::Deserialize;
use slack_morphism::api::{
    SlackApiChatDeleteRequest, SlackApiChatUpdateRequest, SlackApiPinsAddRequest,
    SlackApiPinsAddResponse, SlackApiPinsRemoveRequest, SlackApiPinsRemoveResponse,
    SlackApiReactionsAddRequest, SlackApiReactionsAddResponse, SlackApiReactionsRemoveRequest,
    SlackApiReactionsRemoveResponse,
};
use slack_morphism::{SlackChannelId, SlackClientHttpConnector, SlackMessageContent, SlackTs};

#[derive(Deserialize)]
struct PinsResponse {
    items: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct MessageAcknowledgement {
    channel: SlackChannelId,
    ts: SlackTs,
}

impl<C: SlackClientHttpConnector + Send + Sync> Slack<C> {
    pub(crate) async fn pins(
        &self,
        credentials: &Credentials,
        channel: &SlackChannelId,
    ) -> Result<Vec<serde_json::Value>> {
        let response: PinsResponse = self
            .get(
                credentials,
                "pins.list",
                vec![("channel", Some(&channel.0))],
            )
            .await?;
        Ok(response.items)
    }

    pub(crate) async fn change(
        &self,
        credentials: &Credentials,
        message: Message,
        change: Change,
    ) -> Result<Changed> {
        let acknowledgement: Option<MessageAcknowledgement> = match &change {
            Change::Edit { content } => {
                let request = SlackApiChatUpdateRequest::new(
                    message.channel_id.clone(),
                    SlackMessageContent::from(content),
                    message.ts.clone(),
                )
                .with_link_names(false)
                .with_parse("none".to_owned());
                Some(self.write(credentials, "chat.update", &request).await?)
            }
            Change::Delete => {
                let request =
                    SlackApiChatDeleteRequest::new(message.channel_id.clone(), message.ts.clone());
                Some(self.write(credentials, "chat.delete", &request).await?)
            }
            Change::React { name } => {
                let request = SlackApiReactionsAddRequest::new(
                    message.channel_id.clone(),
                    name.clone(),
                    message.ts.clone(),
                );
                let _: SlackApiReactionsAddResponse =
                    self.write(credentials, "reactions.add", &request).await?;
                None
            }
            Change::Unreact { name } => {
                let request = SlackApiReactionsRemoveRequest::new(name.clone())
                    .with_channel(message.channel_id.clone())
                    .with_timestamp(message.ts.clone());
                let _: SlackApiReactionsRemoveResponse = self
                    .write(credentials, "reactions.remove", &request)
                    .await?;
                None
            }
            Change::Pin => {
                let request =
                    SlackApiPinsAddRequest::new(message.channel_id.clone(), message.ts.clone());
                let _: SlackApiPinsAddResponse =
                    self.write(credentials, "pins.add", &request).await?;
                None
            }
            Change::Unpin => {
                let request =
                    SlackApiPinsRemoveRequest::new(message.channel_id.clone(), message.ts.clone());
                let _: SlackApiPinsRemoveResponse =
                    self.write(credentials, "pins.remove", &request).await?;
                None
            }
        };
        if acknowledgement.is_some_and(|response| {
            response.channel != message.channel_id || response.ts != message.ts
        }) {
            return Err(Error::WriteOutcomeUnknown { slack_error: None });
        }
        Ok(Changed {
            channel_id: message.channel_id,
            ts: message.ts,
            change,
        })
    }
}
