use super::{identity_text, terminal_text};
use crate::auth::ProfileView;
use crate::slack::ContentType;
use serde::Serialize;
use slack_morphism::SlackCanvasId;
use slack_morphism::api::{SlackApiCanvasesEditRequest, SlackCanvasSection};

#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum Output {
    Content {
        canvas_id: SlackCanvasId,
        content_type: ContentType,
        content: String,
    },
    Sections {
        canvas_id: SlackCanvasId,
        sections: Vec<SlackCanvasSection>,
    },
    Preview {
        dry_run: bool,
        #[serde(flatten)]
        profile: Box<ProfileView>,
        before: String,
        #[serde(flatten)]
        request: SlackApiCanvasesEditRequest,
    },
    Edited {
        #[serde(flatten)]
        request: SlackApiCanvasesEditRequest,
    },
}

impl Output {
    pub(super) fn text(&self) -> String {
        match self {
            Self::Content {
                canvas_id,
                content_type,
                content,
            } => format!(
                "Canvas: {}\nFormat: {:?}\n{}",
                terminal_text(&canvas_id.0),
                content_type,
                terminal_text(content)
            ),
            Self::Sections {
                canvas_id,
                sections,
            } => format!(
                "Canvas: {}\nMatching sections (not a complete section list):\n{}",
                terminal_text(&canvas_id.0),
                sections
                    .iter()
                    .map(|section| terminal_text(&section.id.0))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
            Self::Preview {
                profile,
                before,
                request,
                ..
            } => format!(
                "Dry run; no canvas edit was made.\n{}",
                preview(profile, before, request)
            ),
            Self::Edited { request } => format!("Acknowledged canvas edit\n{}", edit_text(request)),
        }
    }
}

pub(crate) fn preview(
    profile: &ProfileView,
    before: &str,
    request: &SlackApiCanvasesEditRequest,
) -> String {
    format!(
        "{}\nCurrent full canvas (Markdown):\n{}\n{}\nSlack validates the explicit section ID. This preview does not lock the canvas or prevent concurrent edits. No automatic rollback.",
        identity_text(profile),
        terminal_text(before),
        edit_text(request)
    )
}

fn edit_text(request: &SlackApiCanvasesEditRequest) -> String {
    let changes = request
        .changes
        .iter()
        .map(|change| {
            format!(
                "Operation: {:?}\nSection: {}\nMarkdown:\n{}",
                change.operation,
                change.section_id.as_ref().map_or_else(
                    || "not applicable".to_owned(),
                    |section| terminal_text(&section.0)
                ),
                change.document_content.as_ref().map_or_else(
                    || "not applicable".to_owned(),
                    |content| terminal_text(&content.markdown)
                )
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("Canvas: {}\n{changes}", terminal_text(&request.canvas_id.0))
}
