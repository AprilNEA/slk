use super::conversations::{CONVERSATION_TYPES, WireMessage};
use super::pagination::{self, RequestPage};
use super::{Page, Pagination, Slack};
use crate::auth::{Credentials, Kind};
use crate::error::{Error, Result};
use crate::model::messages::{MAX_SEARCH_PAGE, Message, SearchPage};
use serde::Deserialize;
use slack_morphism::{
    SlackApiToken, SlackBasicChannelInfo, SlackChannelId, SlackClientHttpConnector,
    SlackResponseMetadata, SlackTs,
};

impl RequestPage for SearchPage {
    fn max_pages(&self) -> u16 {
        self.max_pages
    }

    fn advance(&mut self, pagination: &Pagination) -> Result<bool> {
        match pagination {
            Pagination::Cursor { next_cursor, .. } => {
                self.cursor.clone_from(next_cursor);
                Ok(self.cursor.is_some())
            }
            Pagination::Number { next_page, .. } => {
                self.page = *next_page;
                Ok(self.page.is_some())
            }
        }
    }
}

#[derive(Deserialize)]
struct ContextResponse {
    results: ContextResults,
    response_metadata: Option<SlackResponseMetadata>,
}

#[derive(Deserialize)]
struct ContextResults {
    messages: Vec<ContextMessage>,
}

#[derive(Deserialize)]
struct ContextMessage {
    channel_id: SlackChannelId,
    message_ts: SlackTs,
    thread_ts: Option<SlackTs>,
    author_user_id: Option<String>,
    content: String,
    permalink: Option<String>,
}

#[derive(Deserialize)]
struct SearchResponse {
    messages: SearchMessages,
}

#[derive(Deserialize)]
struct SearchMessages {
    matches: Vec<SearchMatch>,
    paging: Paging,
}

#[derive(Deserialize)]
struct SearchMatch {
    channel: SlackBasicChannelInfo,
    #[serde(flatten)]
    message: WireMessage,
}

#[derive(Deserialize)]
struct Paging {
    page: u32,
    pages: u32,
}

impl<C: SlackClientHttpConnector + Send + Sync> Slack<C> {
    pub(crate) async fn search(
        &self,
        credentials: &Credentials,
        query: &str,
        page: &SearchPage,
    ) -> Result<Page<Message>> {
        page.validate(credentials.kind(), credentials.realm())?;
        if query.trim().is_empty() {
            return Err(Error::Arguments("The search query must not be empty."));
        }
        pagination::collect(page, |page| async move {
            match credentials.kind() {
                Kind::Bot => Err(Error::Capability(
                    "Slack search does not accept bot identities.",
                )),
                Kind::User => {
                    let token = SlackApiToken::from(credentials);
                    let session = self.client.open_session(&token);
                    let mut body = serde_json::json!({
                        "query": query,
                        "limit": page.limit,
                        "channel_types": CONVERSATION_TYPES.split(',').collect::<Vec<_>>(),
                        "content_types": ["messages"],
                        "include_bots": true,
                        "include_context_messages": false,
                        "include_message_blocks": false,
                        "highlight": false
                    });
                    if let Some(cursor) = &page.cursor {
                        body["cursor"] = cursor.clone().into();
                    }
                    let response: ContextResponse = self
                        .response(session.http_session_api.http_post(
                            "assistant.search.context",
                            &body,
                            None,
                        ))
                        .await?;
                    let messages = response
                        .results
                        .messages
                        .into_iter()
                        .map(|message| Message {
                            channel_id: message.channel_id,
                            ts: message.message_ts,
                            thread_ts: message.thread_ts,
                            author: message.author_user_id,
                            text: message.content,
                            blocks: None,
                            permalink: message.permalink,
                        })
                        .collect();
                    Ok(Page::cursor(messages, response.response_metadata, None))
                }
                Kind::Browser => {
                    let requested_page = page.page.unwrap_or(1);
                    let response: SearchResponse = self
                        .get(
                            credentials,
                            "search.messages",
                            vec![
                                ("query", Some(query)),
                                ("count", Some(&page.limit.to_string())),
                                ("page", Some(&requested_page.to_string())),
                                ("highlight", Some("false")),
                            ],
                        )
                        .await?;
                    let paging = response.messages.paging;
                    if paging.page != requested_page {
                        return Err(Error::Response);
                    }
                    Ok(Page {
                        data: response
                            .messages
                            .matches
                            .into_iter()
                            .map(|matched| matched.message.in_channel(matched.channel.id))
                            .collect(),
                        pagination: Pagination::Number {
                            next_page: (paging.page < paging.pages
                                && paging.page < MAX_SEARCH_PAGE)
                                .then(|| paging.page + 1),
                            has_more: paging.page < paging.pages,
                        },
                    })
                }
            }
        })
        .await
    }
}
