use super::{Approval, Connection, Prompt};
use crate::error::Result;
use crate::model::canvas::ContentType;
use crate::output::canvas::{Output, preview};
use slack_morphism::api::{SlackApiCanvasesEditRequest, SlackApiCanvasesSectionsLookupRequest};
use slack_morphism::{SlackCanvasId, SlackClientHttpConnector};

pub(crate) enum Request {
    Read {
        canvas_id: SlackCanvasId,
        content_type: ContentType,
    },
    Sections(SlackApiCanvasesSectionsLookupRequest),
    Edit {
        request: SlackApiCanvasesEditRequest,
        approval: Approval,
    },
}

impl Request {
    pub(crate) async fn execute<C: SlackClientHttpConnector + Send + Sync>(
        self,
        connection: Connection<C>,
        prompt: &dyn Prompt,
    ) -> Result<Output> {
        let Connection {
            slack,
            credentials,
            profile,
        } = connection;
        let credentials = &credentials;
        Ok(match self {
            Self::Read {
                canvas_id,
                content_type,
            } => Output::Content {
                content: slack
                    .canvas_content(credentials, &canvas_id, content_type)
                    .await?,
                canvas_id,
                content_type,
            },
            Self::Sections(request) => Output::Sections {
                sections: slack.canvas_sections(credentials, &request).await?,
                canvas_id: request.canvas_id,
            },
            Self::Edit { request, approval } => {
                let before = slack
                    .canvas_content(credentials, &request.canvas_id, ContentType::Markdown)
                    .await?;
                if !approval.confirm(prompt, "Apply this canvas edit?", || {
                    preview(&profile, &before, &request)
                })? {
                    Output::Preview {
                        dry_run: true,
                        profile: Box::new(profile),
                        before,
                        request,
                    }
                } else {
                    slack.edit_canvas(credentials, &request).await?;
                    Output::Edited { request }
                }
            }
        })
    }
}
