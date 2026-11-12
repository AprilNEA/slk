use crate::error::{Error, Result};
use clap::ValueEnum;
use serde::Serialize;
use slack_morphism::api::{
    SlackApiCanvasesEditRequest, SlackCanvasChange, SlackCanvasDocumentContent,
    SlackCanvasOperation,
};
use slack_morphism::{SlackCanvasId, SlackCanvasSectionId};

const MAX_MARKDOWN_BYTES: usize = 1_048_576;

#[derive(Clone, Copy, Debug, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ContentType {
    Markdown,
    Html,
}

/// Builds the edit that writes `markdown` with `operation`.
///
/// `section_id` anchors the operations that target one section.
pub(crate) fn markdown_edit(
    canvas_id: SlackCanvasId,
    operation: SlackCanvasOperation,
    section_id: Option<SlackCanvasSectionId>,
    markdown: String,
) -> Result<SlackApiCanvasesEditRequest> {
    if markdown.trim().is_empty() || markdown.len() > MAX_MARKDOWN_BYTES {
        return Err(Error::Arguments(
            "Canvas Markdown must be nonempty and at most 1 MiB of UTF-8. Content is never split or truncated.",
        ));
    }
    let change = SlackCanvasChange::new(operation)
        .opt_section_id(section_id)
        .with_document_content(SlackCanvasDocumentContent::new(markdown));
    Ok(SlackApiCanvasesEditRequest::new(canvas_id, vec![change]))
}

/// Builds the edit that deletes one section.
pub(crate) fn section_deletion(
    canvas_id: SlackCanvasId,
    section_id: SlackCanvasSectionId,
) -> SlackApiCanvasesEditRequest {
    SlackApiCanvasesEditRequest::new(
        canvas_id,
        vec![SlackCanvasChange::new(SlackCanvasOperation::Delete).with_section_id(section_id)],
    )
}
