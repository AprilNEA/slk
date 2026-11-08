use crate::cli::auth::AuthCommand;
use crate::cli::canvas::CanvasCommand;
use crate::cli::compat;
use crate::cli::conversations::{ChannelCommand, ConversationCommand, GroupCommand};
use crate::cli::files::FileCommand;
use crate::cli::messages::MessageCommand;
use crate::cli::scheduled::ScheduledCommand;
use crate::cli::users::UserCommand;
use crate::cli::write::WriteInput;
use crate::cli::{Cli, Command};
use crate::error::{Error, Result};
use crate::output::Output;
use base64::{Engine, prelude::BASE64_STANDARD};
use clap::{CommandFactory, FromArgMatches};
use rmcp::{RoleServer, service::RequestContext};
use std::collections::BTreeMap;
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

pub(super) struct Request {
    cli: Cli,
    directory: Option<TempDir>,
    mutating: bool,
}

impl Request {
    pub(super) fn read(profile: &str, args: Vec<String>) -> Result<Self> {
        let (cli, _) = parse(profile, args)?;
        if !read_only(&cli.command) {
            return Err(Error::Arguments(
                "MCP accepts read-only commands. Writes, previews, file transfers, authentication changes, and nested servers are unavailable.",
            ));
        }
        Ok(Self {
            cli,
            directory: None,
            mutating: false,
        })
    }

    pub(super) fn write(
        profile: &str,
        args: Vec<String>,
        files: BTreeMap<String, String>,
    ) -> Result<Self> {
        let (mut cli, preview) = parse(profile, args)?;
        if read_only(&cli.command)
            || matches!(
                cli.command,
                Command::Mcp(_) | Command::Update(_) | Command::Auth { .. }
            )
        {
            return Err(Error::Arguments(
                "slk_write accepts Slack writes, previews, and file transfers only. Use slk_read for reads; manage authentication and the server separately.",
            ));
        }
        let mutating = !preview
            && !matches!(
                cli.command,
                Command::Files {
                    command: FileCommand::Download { .. },
                    ..
                }
            );
        let directory = tempfile::tempdir()
            .map_err(|error| Error::io("create temporary MCP file storage", error))?;
        let input = |path: &mut PathBuf| {
            let name = filename(path)?;
            if !files.contains_key(name) {
                return Err(Error::Arguments(
                    "Every MCP input filename must appear in files.",
                ));
            }
            *path = directory.path().join(name);
            Ok(())
        };
        let message = |value: &mut WriteInput| -> Result<()> {
            for path in [&mut value.body.file, &mut value.blocks_file]
                .into_iter()
                .flatten()
            {
                input(path)?;
            }
            Ok(())
        };
        match &mut cli.command {
            Command::Messages(command) => match command.as_mut() {
                MessageCommand::Send { input, .. } | MessageCommand::Reply { input, .. } => {
                    message(&mut input.input)?;
                }
                MessageCommand::Edit { input, .. } => message(input)?,
                MessageCommand::Channels(_)
                | MessageCommand::Search { .. }
                | MessageCommand::Read { .. }
                | MessageCommand::Get { .. }
                | MessageCommand::Thread { .. }
                | MessageCommand::Delete { .. }
                | MessageCommand::React { .. }
                | MessageCommand::Unreact { .. }
                | MessageCommand::Pins { .. }
                | MessageCommand::Pin { .. }
                | MessageCommand::Unpin { .. } => {}
            },
            Command::Compatibility(command) => match command {
                compat::Command::Messages { command, .. } => match command {
                    compat::Message::Send { body, .. } | compat::Message::Edit { body, .. } => {
                        if let Some(path) = &mut body.message_file {
                            input(path)?;
                        }
                    }
                    compat::Message::Delete { .. }
                    | compat::Message::React { .. }
                    | compat::Message::Unreact { .. } => {}
                },
                compat::Command::Conversations { .. } => {}
            },
            Command::Schedule { command } => match command {
                ScheduledCommand::Create { input, .. } | ScheduledCommand::Reply { input, .. } => {
                    message(&mut input.input)?;
                }
                ScheduledCommand::List { .. } | ScheduledCommand::Cancel { .. } => {}
            },
            Command::Canvas { command } => {
                let body = match command {
                    CanvasCommand::Append(input) | CanvasCommand::Prepend(input) => {
                        Some(&mut input.body)
                    }
                    CanvasCommand::InsertBefore(input)
                    | CanvasCommand::InsertAfter(input)
                    | CanvasCommand::ReplaceSection(input) => Some(&mut input.input.body),
                    CanvasCommand::Read { .. }
                    | CanvasCommand::Sections { .. }
                    | CanvasCommand::DeleteSection { .. } => None,
                };
                if let Some(body) = body
                    && let Some(path) = &mut body.file
                {
                    input(path)?;
                }
            }
            Command::Files { command, .. } => match command {
                FileCommand::Upload { path, .. } => input(path)?,
                FileCommand::Download { output, .. } => {
                    *output = directory.path().join(filename(output)?);
                }
                FileCommand::Info { .. } => {}
            },
            Command::Auth { .. }
            | Command::Update(_)
            | Command::Mcp(_)
            | Command::Users(_)
            | Command::Conversations(_) => {}
        }
        for (name, content) in files {
            let path = directory.path().join(filename(Path::new(&name))?);
            let content = BASE64_STANDARD
                .decode(content)
                .map_err(|_| Error::Arguments("MCP file content must use standard base64."))?;
            std::fs::File::options()
                .write(true)
                .create_new(true)
                .open(path)
                .and_then(|mut file| file.write_all(&content))
                .map_err(|error| Error::io("write temporary MCP input files", error))?;
        }
        Ok(Self {
            cli,
            directory: Some(directory),
            mutating,
        })
    }

    pub(super) async fn execute<T>(
        self,
        execute: impl FnOnce(Cli) -> T,
        context: RequestContext<RoleServer>,
    ) -> Result<serde_json::Value>
    where
        T: Future<Output = Result<Output>>,
    {
        let Self {
            cli,
            directory: _directory,
            mutating,
        } = self;
        let mut output = tokio::select! {
            biased;
            () = context.ct.cancelled() => return Err(if mutating {
                Error::WriteOutcomeUnknown { slack_error: None }
            } else {
                Error::Cancelled
            }),
            result = execute(cli) => result?,
        };
        let content = if let Output::Downloaded { path, .. } = &mut output {
            let content = tokio::fs::read(&path)
                .await
                .map_err(|error| Error::io("read the MCP download result", error))?;
            *path = PathBuf::from(
                path.file_name()
                    .expect("MCP downloads use a validated filename"),
            );
            Some(BASE64_STANDARD.encode(content))
        } else {
            None
        };
        let mut value = serde_json::to_value(output)
            .map_err(|error| Error::io("serialize the MCP result", std::io::Error::other(error)))?;
        if let Some(content) = content {
            value["content_base64"] = content.into();
        }
        Ok(value)
    }
}

fn parse(profile: &str, args: Vec<String>) -> Result<(Cli, bool)> {
    let invalid = |_| {
        Error::Arguments(
            "Invalid command arguments. Use slk_help; never supply credentials in tool arguments.",
        )
    };
    let matches = Cli::command()
        .try_get_matches_from(std::iter::once("slk".to_owned()).chain(args))
        .map_err(invalid)?;
    let mut command = &matches;
    while let Some((_, child)) = command.subcommand() {
        command = child;
    }
    let mut cli = Cli::from_arg_matches(&matches).map_err(invalid)?;
    if cli.profile.is_some()
        || cli.source.configured()
        || command.try_contains_id("workspace").unwrap_or(false)
    {
        return Err(Error::Arguments(
            "MCP tools cannot select or override a profile. Configure a separate server for another identity.",
        ));
    }
    let preview = command.try_get_one::<bool>("dry_run").ok().flatten() == Some(&true);
    cli.profile = Some(profile.to_owned());
    cli.json = true;
    Ok((cli, preview))
}

fn read_only(command: &Command) -> bool {
    match command {
        Command::Mcp(_) | Command::Update(_) => false,
        Command::Compatibility(command) => matches!(command, compat::Command::Conversations { .. }),
        Command::Auth { command } => matches!(command, AuthCommand::Status { .. }),
        Command::Messages(command) => matches!(
            command.as_ref(),
            MessageCommand::Channels(_)
                | MessageCommand::Search { .. }
                | MessageCommand::Read { .. }
                | MessageCommand::Get { .. }
                | MessageCommand::Thread { .. }
                | MessageCommand::Pins { .. }
        ),
        Command::Users(command) => matches!(
            command,
            UserCommand::Users { .. } | UserCommand::Resolve { .. }
        ),
        Command::Conversations(command) => match command {
            ConversationCommand::Channel { command } => matches!(
                command,
                ChannelCommand::Info { .. } | ChannelCommand::Members { .. }
            ),
            ConversationCommand::GroupDm { command } => matches!(command, GroupCommand::List(_)),
            ConversationCommand::Unread(_) | ConversationCommand::Workspace => true,
        },
        Command::Files { command, .. } => matches!(command, FileCommand::Info { .. }),
        Command::Schedule { command } => matches!(command, ScheduledCommand::List { .. }),
        Command::Canvas { command } => matches!(
            command,
            CanvasCommand::Read { .. } | CanvasCommand::Sections { .. }
        ),
    }
}

fn filename(path: &Path) -> Result<&str> {
    path.to_str()
        .filter(|name| {
            !name.is_empty()
                && *name != "-"
                && sanitize_filename::is_sanitized_with_options(
                    name,
                    sanitize_filename::OptionsForCheck {
                        windows: true,
                        ..Default::default()
                    },
                )
        })
        .ok_or(Error::Arguments(
            "MCP file arguments must be portable filenames, without directories, device names, or stdin.",
        ))
}
