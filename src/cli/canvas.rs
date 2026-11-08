use super::write::{Approval, BodyInput, WriteOptions};
use crate::auth::{Credentials, ProfileView, Store};
use crate::error::{Error, Result};
use crate::files::parse_file;
use crate::output::{Output, canvas as output};
use crate::slack::{ContentType, Slack};
use clap::{ArgGroup, Args, Subcommand, builder::NonEmptyStringValueParser};
use slack_morphism::api::{
    SlackApiCanvasesEditRequest, SlackApiCanvasesSectionsLookupRequest, SlackCanvasChange,
    SlackCanvasDocumentContent, SlackCanvasOperation, SlackCanvasSectionType,
    SlackCanvasSectionsLookupCriteria,
};
use slack_morphism::{SlackCanvasId, SlackCanvasSectionId, SlackClientHttpConnector};

const MAX_MARKDOWN_BYTES: usize = 1_048_576;

#[derive(Subcommand)]
pub(super) enum CanvasCommand {
    /// Read the full accessible canvas as Markdown or HTML text, without rendering it.
    Read {
        #[arg(value_parser = parse_canvas)]
        canvas_id: SlackCanvasId,
        #[arg(long, value_enum, default_value = "markdown")]
        format: ContentType,
    },
    /// Find matching section IDs; not a guaranteed complete section listing.
    Sections {
        #[arg(value_parser = parse_canvas)]
        canvas_id: SlackCanvasId,
        #[command(flatten)]
        criteria: Criteria,
    },
    /// Append Markdown to the canvas after inspecting its current content.
    Append(EditInput),
    /// Prepend Markdown to the canvas after inspecting its current content.
    Prepend(EditInput),
    /// Insert Markdown before an explicit section ID returned by sections.
    InsertBefore(SectionInput),
    /// Insert Markdown after an explicit section ID returned by sections.
    InsertAfter(SectionInput),
    /// Replace one explicit section, never the entire canvas.
    ReplaceSection(SectionInput),
    /// Delete one explicit section, never the entire canvas.
    DeleteSection {
        #[arg(value_parser = parse_canvas)]
        canvas_id: SlackCanvasId,
        #[arg(value_parser = parse_section)]
        section_id: SlackCanvasSectionId,
        #[command(flatten)]
        options: WriteOptions,
    },
}

#[derive(Args)]
#[command(group(ArgGroup::new("criteria").required(true).multiple(true).args(["contains", "section_type"])))]
pub(super) struct Criteria {
    #[arg(long, value_parser = NonEmptyStringValueParser::new())]
    contains: Option<String>,
    /// Comma-separated Slack section types, such as h1,h2 or any_header.
    #[arg(long, value_delimiter = ',', value_parser = NonEmptyStringValueParser::new())]
    section_type: Vec<String>,
}

#[derive(Args)]
pub(super) struct EditInput {
    #[arg(value_parser = parse_canvas)]
    canvas_id: SlackCanvasId,
    #[command(flatten)]
    pub(super) body: BodyInput,
    #[command(flatten)]
    options: WriteOptions,
}

#[derive(Args)]
pub(super) struct SectionInput {
    #[command(flatten)]
    pub(super) input: EditInput,
    #[arg(value_parser = parse_section)]
    section_id: SlackCanvasSectionId,
}

enum Request {
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

pub(super) struct Prepared {
    pub profile: ProfileView,
    request: Request,
}

impl CanvasCommand {
    pub(super) fn prepare(
        self,
        store: &Store,
        selected: Option<&str>,
        json: bool,
    ) -> Result<Prepared> {
        let profile = store.select(selected)?;
        let edit = |input: EditInput, operation, section_id| -> Result<Request> {
            let approval = input.options.prepare(selected.is_some(), json)?;
            let markdown = input.body.read()?;
            if markdown.trim().is_empty() || markdown.len() > MAX_MARKDOWN_BYTES {
                return Err(Error::Arguments(
                    "Canvas Markdown must be nonempty and at most 1 MiB of UTF-8. Content is never split or truncated.",
                ));
            }
            let change = SlackCanvasChange::new(operation)
                .opt_section_id(section_id)
                .with_document_content(SlackCanvasDocumentContent::new(markdown));
            Ok(Request::Edit {
                request: SlackApiCanvasesEditRequest::new(input.canvas_id, vec![change]),
                approval,
            })
        };
        let request = match self {
            Self::Read { canvas_id, format } => Request::Read {
                canvas_id,
                content_type: format,
            },
            Self::Sections {
                canvas_id,
                criteria,
            } => {
                let types = (!criteria.section_type.is_empty()).then(|| {
                    criteria
                        .section_type
                        .into_iter()
                        .map(SlackCanvasSectionType::Other)
                        .collect()
                });
                Request::Sections(SlackApiCanvasesSectionsLookupRequest::new(
                    canvas_id,
                    SlackCanvasSectionsLookupCriteria::new()
                        .opt_contains_text(criteria.contains)
                        .opt_section_types(types),
                ))
            }
            Self::Append(input) => edit(input, SlackCanvasOperation::InsertAtEnd, None)?,
            Self::Prepend(input) => edit(input, SlackCanvasOperation::InsertAtStart, None)?,
            Self::InsertBefore(SectionInput { input, section_id }) => {
                edit(input, SlackCanvasOperation::InsertBefore, Some(section_id))?
            }
            Self::InsertAfter(SectionInput { input, section_id }) => {
                edit(input, SlackCanvasOperation::InsertAfter, Some(section_id))?
            }
            Self::ReplaceSection(SectionInput { input, section_id }) => {
                edit(input, SlackCanvasOperation::Replace, Some(section_id))?
            }
            Self::DeleteSection {
                canvas_id,
                section_id,
                options,
            } => Request::Edit {
                request: SlackApiCanvasesEditRequest::new(
                    canvas_id,
                    vec![
                        SlackCanvasChange::new(SlackCanvasOperation::Delete)
                            .with_section_id(section_id),
                    ],
                ),
                approval: options.prepare(selected.is_some(), json)?,
            },
        };
        Ok(Prepared { profile, request })
    }
}

impl Prepared {
    pub(super) async fn execute<C: SlackClientHttpConnector + Send + Sync>(
        self,
        slack: &Slack<C>,
        credentials: &Credentials,
    ) -> Result<Output> {
        if slack.identify(credentials).await? != self.profile.identity {
            return Err(Error::IdentityMismatch);
        }
        let result = match self.request {
            Request::Read {
                canvas_id,
                content_type,
            } => output::Output::Content {
                content: slack
                    .canvas_content(credentials, &canvas_id, content_type)
                    .await?,
                canvas_id,
                content_type,
            },
            Request::Sections(request) => output::Output::Sections {
                sections: slack.canvas_sections(credentials, &request).await?,
                canvas_id: request.canvas_id,
            },
            Request::Edit { request, approval } => {
                let before = slack
                    .canvas_content(credentials, &request.canvas_id, ContentType::Markdown)
                    .await?;
                if !approval.confirm("Apply this canvas edit?", || {
                    output::preview(&self.profile, &before, &request)
                })? {
                    output::Output::Preview {
                        dry_run: true,
                        profile: Box::new(self.profile),
                        before,
                        request,
                    }
                } else {
                    slack.edit_canvas(credentials, &request).await?;
                    output::Output::Edited { request }
                }
            }
        };
        Ok(Output::Canvas(result))
    }
}

fn parse_canvas(value: &str) -> Result<SlackCanvasId> {
    Ok(SlackCanvasId(parse_file(value)?.0))
}

fn parse_section(value: &str) -> Result<SlackCanvasSectionId> {
    if value.is_empty()
        || value
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control())
    {
        return Err(Error::Arguments(
            "Use a nonempty section ID returned by canvas sections, without whitespace or controls.",
        ));
    }
    Ok(SlackCanvasSectionId(value.to_owned()))
}

#[cfg(test)]
mod tests;
