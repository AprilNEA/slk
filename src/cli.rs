use crate::app::Connection;
use crate::auth::{Settings, Store};
use crate::error::{Error, Result};
use crate::output::{self, Output};
use crate::slack::{CursorPage, MAX_PAGE_SIZE};
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand, error::ErrorKind};
use std::ffi::OsString;
use std::io;
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
            let cli = Cli::from_arg_matches(&matches)?;
            if cli.source.session_stdin
                && (matches!(cli.command, Command::Mcp(_)) || session::claims_stdin(&matches))
            {
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
    execute_with_store(cli, &mut store).await
}

async fn execute_with_store(cli: Cli, store: &mut Store) -> Result<Output> {
    let command = match cli.command {
        Command::Mcp(_) => return Err(Error::Arguments("Nested MCP servers are not supported.")),
        Command::Update(command) => return command.execute(store.settings(), cli.json).await,
        Command::Auth { command } => command,
        Command::Compatibility(command) => {
            let (profile, prepared) = command.prepare(store, cli.profile.as_deref(), cli.json)?;
            let connection = Connection::open(store, profile).await?;
            return prepared.execute(connection, &write::Terminal).await;
        }
        Command::Messages(command) => {
            let (profile, prepared) = command.prepare(store, cli.profile.as_deref(), cli.json)?;
            let connection = Connection::open(store, profile).await?;
            return prepared.execute(connection, &write::Terminal).await;
        }
        Command::Users(command) => {
            let (profile, prepared) = command.prepare(store, cli.profile.as_deref(), cli.json)?;
            let connection = Connection::open(store, profile).await?;
            return prepared.execute(connection, &write::Terminal).await;
        }
        Command::Conversations(command) => {
            let (profile, prepared) = command.prepare(store, cli.profile.as_deref(), cli.json)?;
            let connection = Connection::open(store, profile).await?;
            return prepared.execute(connection, &write::Terminal).await;
        }
        Command::Files { command, selector } => {
            let selected = selector.resolve(store, cli.profile.as_deref())?;
            let (profile, prepared) = command.prepare(store, selected.as_deref(), cli.json)?;
            let connection = Connection::open(store, profile).await?;
            return prepared.execute(connection, &write::Terminal).await;
        }
        Command::Schedule { command } => {
            let (profile, prepared) = command.prepare(store, cli.profile.as_deref(), cli.json)?;
            let connection = Connection::open(store, profile).await?;
            return prepared.execute(connection, &write::Terminal).await;
        }
        Command::Canvas { command } => {
            let (profile, prepared) = command.prepare(store, cli.profile.as_deref(), cli.json)?;
            let connection = Connection::open(store, profile).await?;
            return prepared.execute(connection, &write::Terminal).await;
        }
    };
    command
        .execute(store, cli.profile, &cli.source, cli.json)
        .await
}
