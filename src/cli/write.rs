use crate::app::{Approval, Prompt};
use crate::auth::Settings;
use crate::error::{Error, Result};
use crate::model::messages::{Content, MAX_TEXT_CHARS, MessageText, Split};
use clap::{ArgGroup, Args};
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

#[derive(Args)]
pub(super) struct WriteInput {
    #[command(flatten)]
    pub(super) body: BodyInput,
    /// Complete Block Kit JSON array; text is the accessible notification fallback.
    #[arg(long, value_name = "PATH")]
    pub(super) blocks_file: Option<PathBuf>,
    #[command(flatten)]
    pub(super) options: WriteOptions,
}

#[derive(Args)]
pub(super) struct SendInput {
    #[command(flatten)]
    pub(super) input: WriteInput,
    /// Split plain text into ordered messages; stop at the first failed or uncertain write.
    #[arg(long, conflicts_with = "blocks_file")]
    pub(super) split: bool,
    /// Unicode characters per chunk; defaults to settings.messages.chunk_chars.
    #[arg(long, requires = "split", value_parser = clap::value_parser!(u16).range(1..=MAX_TEXT_CHARS as i64))]
    pub(super) chunk_chars: Option<u16>,
    /// Reject the entire input before sending if it exceeds this chunk budget.
    #[arg(long, requires = "split", value_parser = clap::value_parser!(u16).range(1..))]
    pub(super) max_chunks: Option<u16>,
}

#[derive(Args)]
#[command(group(ArgGroup::new("body").required(true).args(["text", "file"])))]
pub(super) struct BodyInput {
    /// Body text, never interpreted as a shell command.
    #[arg(long)]
    pub(super) text: Option<String>,
    /// UTF-8 file, or - to read the body from stdin.
    #[arg(long, value_name = "PATH")]
    pub(super) file: Option<PathBuf>,
}

#[derive(Args)]
pub(super) struct WriteOptions {
    /// Verify identity and the target, then preview without making a write request.
    #[arg(long)]
    dry_run: bool,
    /// Skip confirmation; automation also requires an explicit identity selector.
    #[arg(long)]
    yes: bool,
}

impl WriteOptions {
    pub(super) fn prepare(self, explicit_profile: bool, json: bool) -> Result<Approval> {
        let interactive = io::stdin().is_terminal() && io::stderr().is_terminal();
        if self.dry_run {
            Ok(Approval::Preview)
        } else if (json || !interactive) && (!self.yes || !explicit_profile) {
            Err(Error::Arguments(
                "Non-interactive and JSON writes require --yes and an explicit --profile (or --workspace on compatible commands).",
            ))
        } else if self.yes {
            Ok(Approval::Approved)
        } else {
            Ok(Approval::Confirm)
        }
    }
}

impl WriteInput {
    pub(super) fn prepare(self, explicit_profile: bool, json: bool) -> Result<(Content, Approval)> {
        let approval = self.options.prepare(explicit_profile, json)?;
        if self.blocks_file.as_deref() == Some(Path::new("-"))
            && self.body.file.as_deref() == Some(Path::new("-"))
        {
            return Err(Error::Arguments("Text and blocks cannot both read stdin."));
        }
        let text = MessageText::try_from(self.body.read()?)?;
        let blocks = self
            .blocks_file
            .map(|path| {
                let json = BodyInput {
                    text: None,
                    file: Some(path),
                }
                .read()?;
                serde_json::from_str(&json).map_err(|_| {
                    Error::Arguments(
                        "Expected a Block Kit JSON array with at most 50 objects, each with a nonempty string type.",
                    )
                })
            })
            .transpose()?;
        Ok((Content { text, blocks }, approval))
    }
}

impl SendInput {
    pub(super) fn prepare(
        self,
        settings: &Settings,
        explicit_profile: bool,
        json: bool,
    ) -> Result<(Vec<Content>, Approval)> {
        if !self.split {
            let (content, approval) = self.input.prepare(explicit_profile, json)?;
            return Ok((vec![content], approval));
        }
        let approval = self.input.options.prepare(explicit_profile, json)?;
        let split = Split::new(
            self.chunk_chars
                .map_or(settings.messages.chunk_chars.get(), usize::from),
            usize::from(
                self.max_chunks
                    .unwrap_or(settings.messages.max_chunks.get()),
            ),
        )?;
        let content = split.chunks(&self.input.body.read()?)?;
        Ok((content, approval))
    }
}

impl BodyInput {
    pub(super) fn read(self) -> Result<String> {
        let text = match (self.text, self.file) {
            (Some(text), None) => text,
            (None, Some(path)) if path.as_os_str() == "-" => {
                let mut text = String::new();
                io::stdin()
                    .read_to_string(&mut text)
                    .map_err(|error| Error::io("read body text from stdin", error))?;
                text
            }
            (None, Some(path)) => fs::read_to_string(path)
                .map_err(|error| Error::io("read the UTF-8 body file", error))?,
            _ => return Err(Error::Arguments("Provide exactly one of --text or --file.")),
        };
        Ok(text)
    }
}

/// Asks on the controlling terminal; the default answer is no.
pub(super) struct Terminal;

impl Prompt for Terminal {
    fn confirm(&self, question: &str, preview: &str) -> Result<bool> {
        writeln!(io::stderr().lock(), "{preview}")
            .map_err(|error| Error::io("write the operation preview", error))?;
        let confirmed = dialoguer::Confirm::new()
            .with_prompt(question)
            .default(false)
            .wait_for_newline(true)
            .interact_opt()
            .map_err(|_| Error::Confirmation)?;
        Ok(confirmed == Some(true))
    }
}
