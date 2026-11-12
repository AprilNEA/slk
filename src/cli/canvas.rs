use super::write::{BodyInput, WriteOptions};
use super::{Effect, FilePath};
use crate::app::{self, canvas::Request};
use crate::auth::{ProfileView, Store};
use crate::error::Result;
use crate::ids::{parse_canvas, parse_section};
use crate::model::canvas::{ContentType, markdown_edit, section_deletion};
use clap::{ArgGroup, Args, Subcommand, builder::NonEmptyStringValueParser};
use slack_morphism::api::{
    SlackApiCanvasesSectionsLookupRequest, SlackCanvasOperation, SlackCanvasSectionType,
    SlackCanvasSectionsLookupCriteria,
};
use slack_morphism::{SlackCanvasId, SlackCanvasSectionId};

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
    body: BodyInput,
    #[command(flatten)]
    options: WriteOptions,
}

#[derive(Args)]
pub(super) struct SectionInput {
    #[command(flatten)]
    input: EditInput,
    #[arg(value_parser = parse_section)]
    section_id: SlackCanvasSectionId,
}

impl CanvasCommand {
    pub(super) fn effect(&self) -> Effect {
        match self {
            Self::Read { .. } | Self::Sections { .. } => Effect::Read,
            Self::Append(_)
            | Self::Prepend(_)
            | Self::InsertBefore(_)
            | Self::InsertAfter(_)
            | Self::ReplaceSection(_)
            | Self::DeleteSection { .. } => Effect::Write,
        }
    }

    pub(super) fn file_paths(&mut self) -> Vec<FilePath<'_>> {
        match self {
            Self::Append(input) | Self::Prepend(input) => input.body.file_paths(),
            Self::InsertBefore(input) | Self::InsertAfter(input) | Self::ReplaceSection(input) => {
                input.input.body.file_paths()
            }
            Self::Read { .. } | Self::Sections { .. } | Self::DeleteSection { .. } => Vec::new(),
        }
    }

    pub(super) fn prepare(
        self,
        store: &Store,
        selected: Option<&str>,
        json: bool,
    ) -> Result<(ProfileView, app::Request)> {
        let profile = store.select(selected)?;
        let edit = |input: EditInput, operation, section_id| -> Result<Request> {
            let approval = input.options.prepare(selected.is_some(), json)?;
            let markdown = input.body.read()?;
            Ok(Request::Edit {
                request: markdown_edit(input.canvas_id, operation, section_id, markdown)?,
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
                request: section_deletion(canvas_id, section_id),
                approval: options.prepare(selected.is_some(), json)?,
            },
        };
        Ok((profile, app::Request::Canvas(request)))
    }
}

#[cfg(test)]
mod tests;
