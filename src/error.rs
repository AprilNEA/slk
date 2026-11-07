use serde::Serialize;
use std::io;
use thiserror::Error;

pub(crate) type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub(crate) enum Error {
    #[error("{0}")]
    Arguments(&'static str),
    #[error("Profile names must contain only ASCII letters, digits, '.', '_' or '-'.")]
    ProfileName,
    #[error("Invalid credential input. Use the JSON schema for the selected authentication kind.")]
    Credentials,
    #[error(
        "Expected a Slack HTTPS workspace origin without userinfo, a path, or a nonstandard port."
    )]
    Workspace,
    #[error("Could not {action}.")]
    Io {
        action: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("Could not locate the platform configuration directory.")]
    ConfigDirectory,
    #[error("Invalid configuration. Check config.json; existing profiles have not been replaced.")]
    Config,
    #[error(
        "The configuration path must be a regular file inside a real directory, not a symlink."
    )]
    ConfigPath,
    #[error(
        "No profiles configured. Run slk auth login --profile <name> --kind <user|browser|bot>."
    )]
    NoProfiles,
    #[error("Multiple profiles configured. Select one explicitly with --profile.")]
    AmbiguousProfile,
    #[error("The selected profile does not exist.")]
    ProfileNotFound,
    #[error(
        "That profile is bound to another identity or authentication kind. Choose a new profile name."
    )]
    ProfileConflict,
    #[error(
        "The system keyring is unavailable or locked. Check Keychain, Secret Service, or Windows Credential Manager. No plaintext fallback was used."
    )]
    Keyring,
    #[error("Another authentication change is in progress. Wait for that command to finish.")]
    AuthBusy,
    #[error("This profile has no rotating PKCE credentials. Use explicit OAuth login instead.")]
    NotRotating,
    #[error(
        "The profile's credential is missing from the system keyring. Import the credential again."
    )]
    CredentialMissing,
    #[error("The stored credential is malformed. Import the credential again.")]
    StoredCredential,
    #[error(
        "Could not save the profile or remove its new keyring entry. Inspect the slk entries in the system keyring."
    )]
    CredentialCleanup,
    #[error("Slack returned an identity that does not match the selected workspace or profile.")]
    IdentityMismatch,
    #[error(
        "Chrome or Chromium was not found. Install a browser or set settings.browser.executable."
    )]
    BrowserUnavailable,
    #[error(
        "Could not start the login browser. Check settings.browser.executable and the desktop session."
    )]
    BrowserStart,
    #[error("The login browser connection failed. No credentials were saved.")]
    BrowserControl,
    #[error(
        "Could not read the browser session. Retry login explicitly or use manual credential import."
    )]
    BrowserCapture,
    #[error(
        "The login browser was closed before a session was captured. No credentials were saved."
    )]
    BrowserClosed,
    #[error("Browser login was cancelled. No credentials were saved.")]
    BrowserCancelled,
    #[error("Browser login timed out. No credentials were saved.")]
    BrowserTimeout,
    #[error(
        "Browser cleanup failed. Close the login browser and remove its slk-login-* temporary directory. No credentials were saved."
    )]
    BrowserCleanup,
    #[error("Could not open the OAuth authorization page. Use --no-open to open the URL yourself.")]
    OAuthBrowser,
    #[error("OAuth authorization failed or was denied. No credentials were saved.")]
    OAuthDenied,
    #[error("OAuth login was cancelled. No credentials were saved.")]
    OAuthCancelled,
    #[error("OAuth login timed out. No credentials were saved.")]
    OAuthTimeout,
    #[error("The local OAuth callback server failed. No credentials were saved.")]
    OAuthCallback,
    #[error(
        "Slack may have consumed the OAuth grant. Start OAuth login again. No request was retried."
    )]
    OAuthOutcomeUnknown,
    #[error(
        "Slack rotated the credentials, but slk could not verify or save them. Start OAuth login again; the previous refresh token may no longer work."
    )]
    RefreshNotSaved {
        #[source]
        source: Box<Error>,
    },
    #[error(
        "Slack returned a different identity kind. Bot tokens require an explicitly selected bot profile."
    )]
    BotIdentity,
    #[error("{0}")]
    Capability(&'static str),
    #[error("Slack rejected the request.")]
    Slack { slack_error: Option<String> },
    #[error("Slack rate-limited the request. No request was retried.")]
    RateLimited {
        retry_after: Option<u64>,
        slack_error: Option<String>,
    },
    #[error("Slack returned HTTP {status}. No request was retried.")]
    Http { status: u16 },
    #[error("Could not complete the Slack connection. No request was retried.")]
    Transport,
    #[error("The MCP stdio session failed. Restart the MCP client connection.")]
    Mcp,
    #[error(
        "Self-update failed: {0}. No request was retried. If replacement began, verify the installation before running slk again."
    )]
    Update(#[from] self_update::Error),
    #[error("{0}")]
    UpdatePolicy(&'static str),
    #[error("Slack returned an invalid response.")]
    Response,
    #[error("The Slack request timed out. No request was retried.")]
    Timeout,
    #[error("Expected an HTTPS Slack message URL with /archives/<conversation-id>/p<timestamp>.")]
    MessageLink,
    #[error("The message URL belongs to a different workspace than the selected profile.")]
    WorkspaceMismatch,
    #[error(
        "Could not verify the thread root. Supply --thread <root-ts> from Slack and check the message URL."
    )]
    ThreadUnresolved,
    #[error(
        "Could not find the exact message. For a reply, supply its parent with --thread <root-ts>."
    )]
    MessageUnresolved,
    #[error("Editing or deleting requires a message authored by the selected identity.")]
    MessageNotOwned,
    #[error(
        "Only messages authored by the selected user or bot without attachments, files, or event subtypes can be modified. Editing blocks requires --blocks-file; deleting blocks is not supported."
    )]
    MessageNotText,
    #[error("Message text exceeds the single-message limit of {limit} Unicode characters.")]
    MessageTooLong { limit: usize },
    #[error("File exceeds settings.files.max_bytes ({limit} bytes).")]
    FileTooLarge { limit: u64 },
    #[error(
        "Only HTTPS URLs on the exact Slack file host are allowed. No file credentials were sent."
    )]
    FileUrl,
    #[error(
        "Remote files are not downloaded. Slack credentials must not be sent to external hosts."
    )]
    ExternalFile,
    #[error("Slack did not provide the metadata required to download this file.")]
    FileUnavailable,
    #[error(
        "The selected pages do not contain that scheduled message. Inspect schedule list, then use --cursor or bounded --all; no cancellation was attempted."
    )]
    ScheduledNotFound,
    #[error("DM creation requires an active human user, not a deactivated account or app.")]
    DmTarget,
    #[error("The operation was cancelled. No write request was made.")]
    Cancelled,
    #[error("Could not read confirmation from the terminal. No write request was made.")]
    Confirmation,
    #[error(
        "Slack may have applied the operation. Check Slack before repeating it. No request was retried."
    )]
    WriteOutcomeUnknown { slack_error: Option<String> },
    #[error(
        "Split send stopped at chunk {failed_chunk} of {total_chunks}. Check the confirmed receipts and failed chunk before resending; no request was retried."
    )]
    SplitWrite {
        sent: Vec<crate::messages::Sent>,
        failed_chunk: usize,
        total_chunks: usize,
        #[source]
        source: Box<Error>,
    },
}

#[derive(Serialize)]
pub(crate) struct ErrorReport<'a> {
    pub code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slack_error: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch: Option<BatchFailure<'a>>,
}

#[derive(Serialize)]
pub(crate) struct BatchFailure<'a> {
    pub sent: &'a [crate::messages::Sent],
    pub failed_chunk: usize,
    pub total_chunks: usize,
    pub failure: Box<ErrorReport<'a>>,
}

impl Error {
    pub(crate) fn io(action: &'static str, source: io::Error) -> Self {
        Self::Io { action, source }
    }

    pub(crate) fn exit_code(&self) -> u8 {
        match self {
            Self::Arguments(_)
            | Self::ProfileName
            | Self::Credentials
            | Self::Workspace
            | Self::MessageLink
            | Self::WorkspaceMismatch
            | Self::MessageTooLong { .. } => 2,
            _ => 1,
        }
    }

    pub(crate) fn report(&self) -> ErrorReport<'_> {
        let code = match self {
            Self::Arguments(_) | Self::ProfileName => "invalid_arguments",
            Self::Credentials => "invalid_credentials",
            Self::Workspace => "invalid_workspace",
            Self::Io { .. } => "io_error",
            Self::ConfigDirectory => "config_directory_unavailable",
            Self::Config => "invalid_config",
            Self::ConfigPath => "unsafe_config_path",
            Self::NoProfiles => "no_profiles",
            Self::AmbiguousProfile => "profile_required",
            Self::ProfileNotFound => "profile_not_found",
            Self::ProfileConflict => "profile_conflict",
            Self::Keyring => "keyring_unavailable",
            Self::AuthBusy => "authentication_busy",
            Self::NotRotating => "not_rotating",
            Self::CredentialMissing => "credential_missing",
            Self::StoredCredential => "invalid_stored_credential",
            Self::CredentialCleanup => "credential_cleanup_failed",
            Self::IdentityMismatch => "identity_mismatch",
            Self::BrowserUnavailable => "browser_unavailable",
            Self::BrowserStart => "browser_start_failed",
            Self::BrowserControl => "browser_connection_failed",
            Self::BrowserCapture => "browser_capture_failed",
            Self::BrowserClosed => "browser_closed",
            Self::BrowserCancelled => "browser_login_cancelled",
            Self::BrowserTimeout => "browser_login_timeout",
            Self::BrowserCleanup => "browser_cleanup_failed",
            Self::OAuthBrowser => "oauth_browser_failed",
            Self::OAuthDenied => "oauth_denied",
            Self::OAuthCancelled => "oauth_cancelled",
            Self::OAuthTimeout => "oauth_timeout",
            Self::OAuthCallback => "oauth_callback_failed",
            Self::OAuthOutcomeUnknown => "oauth_outcome_unknown",
            Self::RefreshNotSaved { .. } => "oauth_refresh_not_saved",
            Self::BotIdentity => "unsupported_identity",
            Self::Capability(_) => "unsupported_capability",
            Self::Slack { .. } => "slack_error",
            Self::RateLimited { .. } => "rate_limited",
            Self::Http { .. } => "http_error",
            Self::Transport => "transport_error",
            Self::Mcp => "mcp_transport_error",
            Self::Update(_) => "update_failed",
            Self::UpdatePolicy(_) => "invalid_release",
            Self::Response => "invalid_response",
            Self::Timeout => "request_timeout",
            Self::MessageLink => "invalid_message_url",
            Self::WorkspaceMismatch => "workspace_mismatch",
            Self::ThreadUnresolved => "thread_unresolved",
            Self::MessageUnresolved => "message_unresolved",
            Self::MessageNotOwned => "message_not_owned",
            Self::MessageNotText => "unsupported_message",
            Self::MessageTooLong { .. } => "message_too_long",
            Self::FileTooLarge { .. } => "file_too_large",
            Self::FileUrl => "unsafe_file_url",
            Self::ExternalFile => "external_file",
            Self::FileUnavailable => "file_unavailable",
            Self::ScheduledNotFound => "scheduled_message_not_found",
            Self::DmTarget => "unsupported_dm_target",
            Self::Cancelled => "cancelled",
            Self::Confirmation => "confirmation_failed",
            Self::WriteOutcomeUnknown { .. } => "write_outcome_unknown",
            Self::SplitWrite { .. } => "split_write_failed",
        };
        let (slack_error, retry_after) = match self {
            Self::Slack { slack_error } | Self::WriteOutcomeUnknown { slack_error } => {
                (slack_error.as_deref(), None)
            }
            Self::RateLimited {
                slack_error,
                retry_after,
            } => (slack_error.as_deref(), *retry_after),
            _ => (None, None),
        };
        let message = if matches!(self, Self::Slack { .. })
            && matches!(
                slack_error,
                Some("invalid_auth" | "not_authed" | "token_revoked" | "token_expired")
            ) {
            "Slack rejected the credentials. Use slk auth refresh for rotating PKCE credentials, or explicitly reauthorize with slk auth oauth, slk auth browser, or slk auth login for the selected profile. No request was retried."
                .to_owned()
        } else {
            self.to_string()
        };
        ErrorReport {
            code,
            message,
            slack_error,
            retry_after,
            batch: match self {
                Self::SplitWrite {
                    sent,
                    failed_chunk,
                    total_chunks,
                    source,
                } => Some(BatchFailure {
                    sent,
                    failed_chunk: *failed_chunk,
                    total_chunks: *total_chunks,
                    failure: Box::new(source.report()),
                }),
                _ => None,
            },
        }
    }
}
