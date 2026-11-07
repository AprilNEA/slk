use super::{identity_text, terminal_text};
use crate::auth::ProfileView;
use crate::messages::{Change, Content, Draft, Message};
use slack_morphism::prelude::ValueStruct;

pub(super) fn message_text(message: &Message) -> String {
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

pub(super) fn change_text(change: &Change) -> String {
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
