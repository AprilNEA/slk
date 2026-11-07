use crate::error::{Error, Result};
use clap::ValueEnum;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use slack_morphism::{SlackApiCookieValue, SlackApiToken, SlackApiTokenValue};
use std::fmt;
use url::Url;
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    User,
    Browser,
    Bot,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Realm {
    #[default]
    Commercial,
    Gov,
}

impl Realm {
    pub(crate) fn domain(self) -> &'static str {
        match self {
            Self::Commercial => "slack.com",
            Self::Gov => "slack-gov.com",
        }
    }

    pub(crate) fn api_url(self) -> String {
        format!("https://{}/api", self.domain())
    }

    pub(crate) fn authorization_url(self) -> String {
        format!("https://{}/oauth/v2/authorize", self.domain())
    }

    pub(crate) fn file_host(self) -> String {
        format!("files.{}", self.domain())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "String")]
pub(crate) struct Workspace(Url);

impl TryFrom<String> for Workspace {
    type Error = Error;

    fn try_from(value: String) -> Result<Self> {
        let url = Url::parse(&value).map_err(|_| Error::Workspace)?;
        let label = [Realm::Commercial, Realm::Gov]
            .into_iter()
            .find_map(|realm| {
                url.host_str()?
                    .strip_suffix(&format!(".{}", realm.domain()))
            })
            .ok_or(Error::Workspace)?;
        if value.trim() != value
            || value.chars().any(char::is_control)
            || value.contains(['@', '\\'])
            || url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
            || label.is_empty()
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(Error::Workspace);
        }
        Ok(Self(url))
    }
}

impl fmt::Display for Workspace {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Workspace {
    pub(crate) fn api_url(&self) -> String {
        format!("{}api", self.0)
    }

    pub(crate) fn realm(&self) -> Realm {
        if self
            .0
            .host_str()
            .is_some_and(|host| host.ends_with(Realm::Gov.domain()))
        {
            Realm::Gov
        } else {
            Realm::Commercial
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Credentials {
    User {
        token: SecretString,
        rotation: Option<Rotation<SecretString>>,
        realm: Realm,
    },
    Bot {
        token: SecretString,
        realm: Realm,
    },
    Browser {
        workspace: Workspace,
        token: SecretString,
        cookie: SecretString,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Rotation<T> {
    pub client_id: String,
    pub refresh_token: T,
    pub expires_at: u64,
    pub scope: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CredentialInput<T> {
    token: T,
    #[serde(skip_serializing_if = "Option::is_none")]
    realm: Option<Realm>,
    #[serde(skip_serializing_if = "Option::is_none")]
    workspace_url: Option<Workspace>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cookie: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rotation: Option<Rotation<T>>,
}

impl Credentials {
    pub(crate) fn parse(kind: Kind, input: &str) -> Result<Self> {
        let input: CredentialInput<SecretString> =
            serde_json::from_str(input).map_err(|_| Error::Credentials)?;
        Self::from_input(kind, input)
    }

    pub(super) fn from_input(kind: Kind, input: CredentialInput<SecretString>) -> Result<Self> {
        let mut credentials = Self::new(kind, input.token, input.workspace_url, input.cookie)?;
        if let Some(realm) = input.realm {
            credentials = credentials.with_realm(realm)?;
        }
        credentials.with_rotation(input.rotation)
    }

    pub(crate) fn new(
        kind: Kind,
        token: SecretString,
        workspace: Option<Workspace>,
        cookie: Option<SecretString>,
    ) -> Result<Self> {
        let prefixes: &[&str] = match kind {
            Kind::User => &["xoxp-", "xoxe.xoxp-"],
            Kind::Browser => &["xoxc-"],
            Kind::Bot => &["xoxb-", "xoxe.xoxb-"],
        };
        if !valid_token(token.expose_secret(), prefixes) {
            return Err(Error::Credentials);
        }
        match (kind, workspace, cookie) {
            (Kind::User, None, None) => Ok(Self::User {
                token,
                rotation: None,
                realm: Realm::default(),
            }),
            (Kind::Bot, None, None) => Ok(Self::Bot {
                token,
                realm: Realm::default(),
            }),
            (Kind::Browser, Some(workspace), Some(cookie)) => {
                let value = cookie.expose_secret();
                if !value.starts_with("xoxd-")
                    || value.len() == "xoxd-".len()
                    || !value
                        .bytes()
                        .all(|byte| byte.is_ascii_graphic() && !b";,\"\\".contains(&byte))
                {
                    return Err(Error::Credentials);
                }
                Ok(Self::Browser {
                    workspace,
                    token,
                    cookie,
                })
            }
            _ => Err(Error::Credentials),
        }
    }

    pub(crate) fn with_realm(mut self, realm: Realm) -> Result<Self> {
        match &mut self {
            Self::User { realm: saved, .. } | Self::Bot { realm: saved, .. } => *saved = realm,
            Self::Browser { workspace, .. } if workspace.realm() == realm => (),
            Self::Browser { .. } => return Err(Error::Credentials),
        }
        Ok(self)
    }

    pub(crate) fn realm(&self) -> Realm {
        match self {
            Self::User { realm, .. } | Self::Bot { realm, .. } => *realm,
            Self::Browser { workspace, .. } => workspace.realm(),
        }
    }

    pub(crate) fn with_rotation(
        mut self,
        rotation: Option<Rotation<SecretString>>,
    ) -> Result<Self> {
        if rotation.as_ref().is_some_and(|rotation| {
            !valid_client_id(&rotation.client_id)
                || !valid_token(rotation.refresh_token.expose_secret(), &["xoxe-"])
        }) {
            return Err(Error::Credentials);
        }
        match (&mut self, rotation) {
            (
                Self::User {
                    rotation: saved, ..
                },
                rotation,
            ) => *saved = rotation,
            (Self::Browser { .. } | Self::Bot { .. }, None) => (),
            (Self::Browser { .. } | Self::Bot { .. }, Some(_)) => return Err(Error::Credentials),
        }
        Ok(self)
    }

    pub(crate) fn rotation(&self) -> Result<&Rotation<SecretString>> {
        match self {
            Self::User {
                rotation: Some(rotation),
                ..
            } => Ok(rotation),
            _ => Err(Error::NotRotating),
        }
    }

    pub(crate) fn kind(&self) -> Kind {
        match self {
            Self::User { .. } => Kind::User,
            Self::Browser { .. } => Kind::Browser,
            Self::Bot { .. } => Kind::Bot,
        }
    }

    pub(crate) fn workspace(&self) -> Option<&Workspace> {
        match self {
            Self::User { .. } | Self::Bot { .. } => None,
            Self::Browser { workspace, .. } => Some(workspace),
        }
    }

    pub(crate) fn encode_for_keyring(&self) -> Result<Zeroizing<String>> {
        let (token, cookie) = match self {
            Self::User { token, .. } | Self::Bot { token, .. } => (token, None),
            Self::Browser { token, cookie, .. } => (token, Some(cookie.expose_secret())),
        };
        let rotation = match self {
            Self::User { rotation, .. } => rotation.as_ref().map(|rotation| Rotation {
                client_id: rotation.client_id.clone(),
                refresh_token: rotation.refresh_token.expose_secret(),
                expires_at: rotation.expires_at,
                scope: rotation.scope.clone(),
            }),
            Self::Browser { .. } | Self::Bot { .. } => None,
        };
        let input = CredentialInput {
            token: token.expose_secret(),
            realm: (self.realm() != Realm::Commercial).then_some(self.realm()),
            workspace_url: self.workspace().cloned(),
            cookie,
            rotation,
        };
        serde_json::to_string(&input)
            .map(Zeroizing::new)
            .map_err(|_| Error::Credentials)
    }
}

impl From<&Credentials> for SlackApiToken {
    fn from(credentials: &Credentials) -> Self {
        let (token, cookie) = match credentials {
            Credentials::User { token, .. } | Credentials::Bot { token, .. } => (token, None),
            Credentials::Browser { token, cookie, .. } => (token, Some(cookie)),
        };
        Self::new(SlackApiTokenValue(token.expose_secret().to_owned())).opt_cookie(
            cookie.map(|value| SlackApiCookieValue(format!("d={}", value.expose_secret()))),
        )
    }
}

pub(super) fn valid_client_id(value: &str) -> bool {
    value.split_once('.').is_some_and(|(first, second)| {
        [first, second]
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    })
}

fn valid_token(value: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|prefix| {
        value.strip_prefix(prefix).is_some_and(|suffix| {
            !suffix.is_empty()
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
    })
}

#[cfg(test)]
mod tests;
