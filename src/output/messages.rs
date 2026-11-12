use super::{identity_text, page_text, terminal_text};
use crate::auth::ProfileView;
use crate::model::messages::{Change, Changed, Content, Draft, Message, Sent};
use crate::slack::Page;
use serde::Serialize;
use slack_morphism::SlackChannelId;
use slack_morphism::prelude::ValueStruct;

#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum Output {
    Messages(Page<Message>),
    Message(Message),
    Pins {
        channel_id: SlackChannelId,
        data: Vec<serde_json::Value>,
    },
    Preview {
        dry_run: bool,
        #[serde(flatten)]
        profile: ProfileView,
        #[serde(flatten)]
        draft: Draft,
    },
    Sent(Sent),
    BatchPreview {
        dry_run: bool,
        #[serde(flatten)]
        profile: ProfileView,
        drafts: Vec<Draft>,
    },
    SentBatch {
        data: Vec<Sent>,
    },
    ChangePreview {
        dry_run: bool,
        #[serde(flatten)]
        profile: ProfileView,
        message: Message,
        #[serde(flatten)]
        change: Change,
    },
    Changed(Changed),
}

impl Output {
    pub(super) fn text(&self) -> String {
        match self {
            Self::Messages(page) => {
                let rows = page.data.iter().map(message_text).collect();
                page_text(rows, &page.pagination)
            }
            Self::Message(message) => message_text(message),
            Self::Pins { channel_id, data } => format!(
                "Channel pins: {}\n{}",
                terminal_text(channel_id.value()),
                if data.is_empty() {
                    "No pins.".to_owned()
                } else {
                    data.iter()
                        .map(|item| terminal_text(&item.to_string()))
                        .collect::<Vec<_>>()
                        .join("\n")
                },
            ),
            Self::Preview { profile, draft, .. } => {
                format!(
                    "Dry run; no message was sent.\n{}",
                    preview_text(profile, draft)
                )
            }
            Self::Sent(sent) => format!(
                "Sent\t{}\t{}\tthread:{}",
                terminal_text(sent.channel_id.value()),
                terminal_text(sent.ts.value()),
                terminal_text(sent.thread_ts.as_ref().map_or("-", |ts| ts.value()))
            ),
            Self::BatchPreview {
                profile, drafts, ..
            } => format!(
                "Dry run; no messages were sent.\n{}",
                batch_preview_text(profile, drafts),
            ),
            Self::SentBatch { data } => format!(
                "Sent {} messages in order.\n{}",
                data.len(),
                terminal_text(
                    &serde_json::to_string(data).expect("receipts contain serializable strings")
                )
            ),
            Self::ChangePreview {
                profile,
                message,
                change,
                ..
            } => format!(
                "Dry run; no change was made.\n{}",
                change_preview_text(profile, message, change)
            ),
            Self::Changed(result) => format!(
                "Applied\t{}\t{}\n{}",
                terminal_text(result.channel_id.value()),
                terminal_text(result.ts.value()),
                change_text(&result.change),
            ),
        }
    }
}

fn message_text(message: &Message) -> String {
    let mut text = format!(
        "{}\t{}\t{}",
        terminal_text(message.channel_id.value()),
        terminal_text(message.ts.value()),
        terminal_text(message.author.as_deref().unwrap_or("-"))
    );
    if let Some(root) = &message.thread_ts {
        text.push_str(&format!("\tthread:{}", terminal_text(root.value())));
    }
    text.push_str(&format!("\n{}", terminal_text(&message.text)));
    if let Some(blocks) = &message.blocks {
        text.push_str(&format!(
            "\nBlock Kit JSON:\n{}",
            terminal_text(&blocks.to_string())
        ));
    }
    if let Some(permalink) = &message.permalink {
        text.push_str(&format!("\n{}", terminal_text(permalink)));
    }
    text
}

pub(super) fn preview_text(profile: &ProfileView, draft: &Draft) -> String {
    format!(
        "{}\nConversation: {}\nThread: {}\n\n{}",
        identity_text(profile),
        terminal_text(draft.channel_id.value()),
        terminal_text(draft.thread_ts.as_ref().map_or("-", |ts| ts.value())),
        content_text(&draft.content),
    )
}

pub(crate) fn batch_preview_text(profile: &ProfileView, drafts: &[Draft]) -> String {
    drafts
        .iter()
        .enumerate()
        .map(|(index, draft)| {
            format!(
                "Message {}/{}\n{}",
                index + 1,
                drafts.len(),
                preview_text(profile, draft)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn content_text(content: &Content) -> String {
    let mut text = terminal_text(content.text.as_ref());
    if let Some(blocks) = &content.blocks {
        let json = serde_json::to_string(blocks).expect("JSON values have infallible serializers");
        text.push_str(&format!(
            "\nBlock Kit JSON (complete array):\n{}",
            terminal_text(&json)
        ));
    }
    text
}

pub(crate) fn change_preview_text(
    profile: &ProfileView,
    message: &Message,
    change: &Change,
) -> String {
    format!(
        "{}\nOriginal message (exact target):\n{}\n{}",
        identity_text(profile),
        message_text(message),
        change_text(change),
    )
}

fn change_text(change: &Change) -> String {
    match change {
        Change::Edit { content } => {
            format!("Action: edit\nNew text:\n{}", content_text(content))
        }
        Change::Delete => "Action: delete this message".to_owned(),
        Change::React { name } => format!("Action: add reaction {}", terminal_text(name.value())),
        Change::Unreact { name } => format!(
            "Action: remove your reaction {}",
            terminal_text(name.value())
        ),
        Change::Pin => {
            "Action: pin this message for the conversation, not personal Later".to_owned()
        }
        Change::Unpin => {
            "Action: unpin this message for the conversation; keep the message".to_owned()
        }
    }
}
