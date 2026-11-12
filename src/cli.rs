use crate::app::Connection;
use crate::auth::{ProfileView, Settings, Store};
use crate::error::{Error, Result};
use crate::model::pagination::{CursorPage, MAX_PAGE_SIZE};
use crate::output::{self, Output};
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand, error::ErrorKind};
use slack_morphism::SlackClientHttpConnector;
use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

mod auth;
mod canvas;
mod compat;
mod conversations;
mod files;
mod mcp;
mod messages;
mod scheduled;
mod session;
mod update;
mod users;
mod write;

#[derive(Parser)]
#[command(
    version,
    about = "A personal Slack CLI for developers and coding agents"
)]
struct Cli {
    /// Select an identity; required for login, logout, and non-interactive writes.
    #[arg(long, global = true)]
    profile: Option<String>,
    /// Write one JSON result, or one JSON error on stderr.
    #[arg(long, global = true)]
    json: bool,
    #[command(flatten)]
    source: session::SessionSource,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve fixed-profile MCP tools over stdio or authenticated HTTP.
    Mcp(mcp::Options),
    /// Check or install a verified stable release from this project's GitHub repository.
    Update(update::UpdateCommand),
    /// Log in, import, inspect, verify, or remove local credentials.
    Auth {
        #[command(subcommand)]
        command: auth::AuthCommand,
    },
    #[command(flatten)]
    Compatibility(compat::Command),
    #[command(flatten)]
    Messages(Box<messages::MessageCommand>),
    #[command(flatten)]
    Users(users::UserCommand),
    #[command(flatten)]
    Conversations(conversations::ConversationCommand),
    /// Inspect, upload, or explicitly download one file.
    Files {
        #[command(flatten)]
        selector: compat::Selector,
        #[command(subcommand)]
        command: files::FileCommand,
    },
    /// Create, list, or cancel Slack service-side scheduled messages.
    Schedule {
        #[command(subcommand)]
        command: scheduled::ScheduledCommand,
    },
    /// Read canvases and apply explicitly targeted Markdown edits.
    Canvas {
        #[command(subcommand)]
        command: canvas::CanvasCommand,
    },
}

/// What a command does to Slack and the local machine. MCP admits commands by their effect.
enum Effect {
    /// Reads Slack or local profile state.
    Read,
    /// Changes Slack state, or previews that change with `--dry-run`.
    Write,
    /// Writes a Slack-hosted file to a local path.
    Download,
    /// Manages credentials, the executable, or a server; no MCP tool accepts the command.
    CliOnly,
}

/// A local file path argument of a command.
enum FilePath<'a> {
    /// Body text or Block Kit JSON to read; `-` reads stdin.
    Body(&'a mut PathBuf),
    /// The file that `files upload` reads.
    Upload(&'a mut PathBuf),
    /// The file that `files download` creates.
    Download(&'a mut PathBuf),
}

impl Command {
    fn effect(&self) -> Effect {
        match self {
            Self::Mcp(_) | Self::Update(_) => Effect::CliOnly,
            Self::Auth { command } => command.effect(),
            Self::Compatibility(command) => command.effect(),
            Self::Messages(command) => command.effect(),
            Self::Users(command) => command.effect(),
            Self::Conversations(command) => command.effect(),
            Self::Files { command, .. } => command.effect(),
            Self::Schedule { command } => command.effect(),
            Self::Canvas { command } => command.effect(),
        }
    }

    fn file_paths(&mut self) -> Vec<FilePath<'_>> {
        match self {
            Self::Mcp(_) | Self::Update(_) => Vec::new(),
            Self::Auth { command } => command.file_paths(),
            Self::Compatibility(command) => command.file_paths(),
            Self::Messages(command) => command.file_paths(),
            Self::Users(command) => command.file_paths(),
            Self::Conversations(command) => command.file_paths(),
            Self::Files { command, .. } => command.file_paths(),
            Self::Schedule { command } => command.file_paths(),
            Self::Canvas { command } => command.file_paths(),
        }
    }

    /// Whether the command reads stdin itself, which a session on stdin would also consume.
    fn claims_stdin(&mut self) -> bool {
        matches!(
            self,
            Self::Mcp(_)
                | Self::Auth {
                    command: auth::AuthCommand::Login { stdin: true, .. }
                }
        ) || self
            .file_paths()
            .iter()
            .any(|path| matches!(path, FilePath::Body(path) if path.as_os_str() == "-"))
    }
}

#[derive(Args)]
struct Paging {
    /// Maximum results per page; defaults to settings.page_size, capped at the API limit.
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..=i64::from(MAX_PAGE_SIZE)))]
    limit: Option<u16>,
    /// Continue from the previous result's next_cursor.
    #[arg(long)]
    cursor: Option<String>,
    #[command(flatten)]
    traversal: Traversal,
}

#[derive(Args)]
struct Traversal {
    /// Follow pages until exhausted or the configured page budget is reached.
    #[arg(long)]
    all: bool,
    /// Maximum page requests with --all; defaults to settings.max_pages.
    #[arg(long, requires = "all", value_parser = clap::value_parser!(u16).range(1..))]
    max_pages: Option<u16>,
}

impl Traversal {
    fn budget(self, settings: &Settings) -> u16 {
        if self.all {
            self.max_pages.unwrap_or(settings.max_pages.get())
        } else {
            1
        }
    }
}

impl Paging {
    fn resolve(self, settings: &Settings) -> CursorPage {
        CursorPage {
            limit: self
                .limit
                .unwrap_or(settings.page_size.get().min(MAX_PAGE_SIZE)),
            cursor: self.cursor,
            max_pages: self.traversal.budget(settings),
        }
    }
}

/// Run the command-line interface using the process arguments and standard streams.
pub async fn run() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().collect();
    let json = args
        .iter()
        .take_while(|arg| *arg != "--")
        .any(|arg| arg == "--json");
    let cli = match Cli::command()
        .try_get_matches_from(args)
        .and_then(|matches| {
            let mut cli = Cli::from_arg_matches(&matches)?;
            if cli.source.session_stdin && cli.command.claims_stdin() {
                return Err(clap::Error::raw(
                    ErrorKind::ArgumentConflict,
                    "Session stdin is already in use.",
                ));
            }
            Ok(cli)
        }) {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            return if error.print().is_ok() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            };
        }
        Err(_) => {
            return fail(
                Error::Arguments(
                    "Invalid command-line arguments. Run slk --help or slk <command> --help. Credentials must never be passed as arguments.",
                ),
                json,
            );
        }
    };
    let json = cli.json;
    if matches!(cli.command, Command::Mcp(_)) {
        return match mcp::run(cli).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => fail(error, json),
        };
    }
    match execute(cli)
        .await
        .and_then(|result| result.write(json, io::stdout().lock()))
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => fail(error, json),
    }
}

fn fail(error: Error, json: bool) -> ExitCode {
    if output::write_error(&error, json).is_err() {
        return ExitCode::FAILURE;
    }
    ExitCode::from(error.exit_code())
}

async fn execute(cli: Cli) -> Result<Output> {
    let mut store = cli.source.load(cli.profile.as_deref())?;
    dispatch(cli, &mut store, Connection::open).await
}

/// Runs one command. `connect` opens the verified connection of a command that calls Slack.
async fn dispatch<C: SlackClientHttpConnector + Send + Sync>(
    cli: Cli,
    store: &mut Store,
    connect: impl AsyncFnOnce(&Store, ProfileView) -> Result<Connection<C>>,
) -> Result<Output> {
    let (profile, request) = match cli.command {
        Command::Mcp(_) => return Err(Error::Arguments("Nested MCP servers are not supported.")),
        Command::Update(command) => return command.execute(store.settings(), cli.json).await,
        Command::Auth { command } => {
            return command
                .execute(store, cli.profile, &cli.source, cli.json)
                .await;
        }
        Command::Compatibility(command) => {
            command.prepare(store, cli.profile.as_deref(), cli.json)?
        }
        Command::Messages(command) => command.prepare(store, cli.profile.as_deref(), cli.json)?,
        Command::Users(command) => command.prepare(store, cli.profile.as_deref(), cli.json)?,
        Command::Conversations(command) => {
            command.prepare(store, cli.profile.as_deref(), cli.json)?
        }
        Command::Files { command, selector } => {
            let selected = selector.resolve(store, cli.profile.as_deref())?;
            command.prepare(store, selected.as_deref(), cli.json)?
        }
        Command::Schedule { command } => {
            command.prepare(store, cli.profile.as_deref(), cli.json)?
        }
        Command::Canvas { command } => command.prepare(store, cli.profile.as_deref(), cli.json)?,
    };
    let connection = connect(store, profile).await?;
    request.execute(connection, &write::Terminal).await
}
