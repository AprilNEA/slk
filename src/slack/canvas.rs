use super::Slack;
use crate::auth::Credentials;
use crate::error::Result;
use crate::model::canvas::ContentType;
use serde::Deserialize;
use serde_json::json;
use slack_morphism::api::{
    SlackApiCanvasesEditRequest, SlackApiCanvasesEditResponse,
    SlackApiCanvasesSectionsLookupRequest, SlackApiCanvasesSectionsLookupResponse,
    SlackCanvasSection,
};
use slack_morphism::{SlackCanvasId, SlackClientHttpConnector};

#[derive(Deserialize)]
struct ContentResponse {
    content: String,
}

impl<C: SlackClientHttpConnector + Send + Sync> Slack<C> {
    pub(crate) async fn canvas_content(
        &self,
        credentials: &Credentials,
        canvas_id: &SlackCanvasId,
        content_type: ContentType,
    ) -> Result<String> {
        let response: ContentResponse = self
            .post(
                credentials,
                "canvases.getContent",
                &json!({"canvas_id": canvas_id, "content_type": content_type}),
            )
            .await?;
        Ok(response.content)
    }

    pub(crate) async fn canvas_sections(
        &self,
        credentials: &Credentials,
        request: &SlackApiCanvasesSectionsLookupRequest,
    ) -> Result<Vec<SlackCanvasSection>> {
        let response: SlackApiCanvasesSectionsLookupResponse = self
            .post(credentials, "canvases.sections.lookup", request)
            .await?;
        Ok(response.sections)
    }

    pub(crate) async fn edit_canvas(
        &self,
        credentials: &Credentials,
        request: &SlackApiCanvasesEditRequest,
    ) -> Result<()> {
        let _: SlackApiCanvasesEditResponse =
            self.write(credentials, "canvases.edit", request).await?;
        Ok(())
    }
}
