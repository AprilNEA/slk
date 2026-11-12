use super::{Approval, Connection, Prompt};
use crate::error::Result;
use crate::output::users::{Output, dm_preview_text};
use crate::slack::CursorPage;
use slack_morphism::{SlackClientHttpConnector, SlackUserId};

pub(crate) enum Request {
    List(CursorPage),
    Info(SlackUserId),
    Lookup(String),
    Resolve {
        name: String,
        page: CursorPage,
    },
    Dm {
        user: SlackUserId,
        approval: Approval,
    },
}

impl Request {
    pub(crate) async fn execute<C: SlackClientHttpConnector + Send + Sync>(
        self,
        connection: Connection<C>,
        prompt: &dyn Prompt,
    ) -> Result<Output> {
        let Connection {
            slack,
            credentials,
            profile,
        } = connection;
        let credentials = &credentials;
        match self {
            Self::List(page) => slack.users(credentials, &page).await.map(Output::Users),
            Self::Info(user) => slack.user(credentials, &user).await.map(Output::User),
            Self::Lookup(email) => slack
                .user_by_email(credentials, &email)
                .await
                .map(Output::User),
            Self::Resolve { name, page } => {
                let mut candidates = slack.users(credentials, &page).await?;
                candidates.data.retain(|user| user.matches_name(&name));
                Ok(Output::ResolvedUsers(candidates.resolve(&page)))
            }
            Self::Dm { user, approval } => {
                let target = slack.user(credentials, &user).await?;
                target.check_dm_target()?;
                if !approval.confirm(prompt, "Open or create this DM?", || {
                    dm_preview_text(&profile, &target)
                })? {
                    return Ok(Output::DmPreview {
                        dry_run: true,
                        profile,
                        target,
                    });
                }
                slack
                    .open_dm(credentials, &target.user_id)
                    .await
                    .map(Output::DmOpened)
            }
        }
    }
}
