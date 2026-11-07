use super::write::Approval;
use crate::auth::Settings;
use crate::error::{Error, Result};
use crate::output::{Output, terminal_text};
use crate::update::{State, Updater};
use clap::Args;
use std::io::{self, IsTerminal};

#[derive(Args)]
pub(super) struct UpdateCommand {
    /// Check the latest stable GitHub release without downloading or replacing the binary.
    #[arg(long, conflicts_with = "yes")]
    check: bool,
    /// Approve replacing the running executable; required for JSON or non-interactive use.
    #[arg(long)]
    yes: bool,
}

impl UpdateCommand {
    pub(super) async fn execute(self, settings: &Settings, json: bool) -> Result<Output> {
        let approval = if self.check {
            Approval::Preview
        } else if self.yes {
            Approval::Approved
        } else if json || !io::stdin().is_terminal() || !io::stderr().is_terminal() {
            return Err(Error::Arguments(
                "Non-interactive updates require --yes. Use --check for discovery without installation.",
            ));
        } else {
            Approval::Confirm
        };
        let updater = Updater::new(settings)?;
        let plan = updater.discover().await?;
        if !plan.available() {
            return Ok(Output::Update(plan.report(State::Current)));
        }
        let path = std::env::current_exe()
            .map_err(|source| Error::io("locate the running executable", source))?;
        let report = plan.report(State::Available);
        if !approval.confirm("Replace this executable?", || format!(
            "Update {} to {}\nTarget: {}\nExecutable: {}\nSHA256 integrity is required. Replacement failures, particularly on Windows, may require manual reinstallation.",
            report.current_version, report.latest_version, report.target, terminal_text(&path.to_string_lossy()),
        ))? {
            return Ok(Output::Update(report));
        }
        updater.install(plan, &path).await.map(Output::Update)
    }
}
