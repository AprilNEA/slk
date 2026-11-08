use crate::cli::auth::AuthCommand;
use crate::cli::{Cli, Command};
use crate::error::{Error, Result};
use crate::output::Output;
use clap::Parser;
use rmcp::{RoleServer, service::RequestContext};
use std::future::Future;

pub(super) struct Request {
    cli: Cli,
}

impl Request {
    pub(super) fn read(profile: &str, args: Vec<String>) -> Result<Self> {
        let mut cli = Cli::try_parse_from(std::iter::once("slk".to_owned()).chain(args))
            .map_err(|_| Error::Arguments("Invalid command arguments. Use slk_help; never supply credentials in tool arguments."))?;
        if cli.profile.is_some() || cli.source.configured() {
            return Err(Error::Arguments(
                "MCP tools cannot select or override a profile. Configure a separate server for another identity.",
            ));
        }
        if !read_only(&cli.command) {
            return Err(Error::Arguments(
                "MCP accepts read-only commands. Writes, previews, file transfers, authentication changes, and nested servers are unavailable.",
            ));
        }
        cli.profile = Some(profile.to_owned());
        cli.json = true;
        Ok(Self { cli })
    }

    pub(super) async fn execute<T>(
        self,
        execute: impl FnOnce(Cli) -> T,
        context: RequestContext<RoleServer>,
    ) -> Result<serde_json::Value>
    where
        T: Future<Output = Result<Output>>,
    {
        let output = tokio::select! {
            biased;
            () = context.ct.cancelled() => return Err(Error::Cancelled),
            result = execute(self.cli) => result?,
        };
        serde_json::to_value(output)
            .map_err(|error| Error::io("serialize the MCP result", std::io::Error::other(error)))
    }
}

fn read_only(command: &Command) -> bool {
    use crate::cli::canvas::CanvasCommand;
    use crate::cli::compat::Selector;
    use crate::cli::conversations::{ChannelCommand, ConversationCommand, GroupCommand};
    use crate::cli::files::FileCommand;
    use crate::cli::messages::MessageCommand;
    use crate::cli::scheduled::ScheduledCommand;
    use crate::cli::users::UserCommand;

    match command {
        Command::Mcp | Command::Update(_) => false,
        Command::Compatibility(command) => command.read_only(),
        Command::Auth { command } => matches!(
            command,
            AuthCommand::Status {
                selector: Selector { workspace: None }
            }
        ),
        Command::Messages(command) => matches!(
            command.as_ref(),
            MessageCommand::Channels(_)
                | MessageCommand::Search {
                    selector: Selector { workspace: None },
                    ..
                }
                | MessageCommand::Read { .. }
                | MessageCommand::Get { .. }
                | MessageCommand::Thread { .. }
                | MessageCommand::Pins { .. }
        ),
        Command::Users(command) => matches!(
            command,
            UserCommand::Users {
                selector: Selector { workspace: None },
                ..
            } | UserCommand::Resolve { .. }
        ),
        Command::Conversations(command) => match command {
            ConversationCommand::Channel { command } => matches!(
                command,
                ChannelCommand::Info { .. } | ChannelCommand::Members { .. }
            ),
            ConversationCommand::GroupDm { command } => matches!(command, GroupCommand::List(_)),
            ConversationCommand::Unread(_) | ConversationCommand::Workspace => true,
        },
        Command::Files { command, selector } => {
            selector.workspace.is_none() && matches!(command, FileCommand::Info { .. })
        }
        Command::Schedule { command } => matches!(command, ScheduledCommand::List { .. }),
        Command::Canvas { command } => matches!(
            command,
            CanvasCommand::Read { .. } | CanvasCommand::Sections { .. }
        ),
    }
}
