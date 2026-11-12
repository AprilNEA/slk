use crate::cli::compat::Selector;
use crate::cli::write::WriteOptions;
use crate::cli::{Cli, Effect, FilePath};
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
        if !matches!(cli.command.effect(), Effect::Read) {
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
        let mutating = match cli.command.effect() {
            Effect::Write => !preview,
            Effect::Download => false,
            Effect::Read | Effect::CliOnly => {
                return Err(Error::Arguments(
                    "slk_write accepts Slack writes, previews, and file transfers only. Use slk_read for reads; manage authentication and the server separately.",
                ));
            }
        };
        let directory = tempfile::tempdir()
            .map_err(|error| Error::io("create temporary MCP file storage", error))?;
        for path in cli.command.file_paths() {
            let (path, input) = match path {
                FilePath::Body(path) | FilePath::Upload(path) => (path, true),
                FilePath::Download(path) => (path, false),
            };
            let name = filename(path)?;
            if input && !files.contains_key(name) {
                return Err(Error::Arguments(
                    "Every MCP input filename must appear in files.",
                ));
            }
            *path = directory.path().join(name);
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
        let content =
            if let Output::Files(crate::output::files::Output::Downloaded { path, .. }) =
                &mut output
            {
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
    if cli.profile.is_some() || cli.source.configured() || Selector::given(command) {
        return Err(Error::Arguments(
            "MCP tools cannot select or override a profile. Configure a separate server for another identity.",
        ));
    }
    let preview = WriteOptions::previews(command);
    cli.profile = Some(profile.to_owned());
    cli.json = true;
    Ok((cli, preview))
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
