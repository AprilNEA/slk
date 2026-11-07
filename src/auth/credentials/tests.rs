use super::{Credentials, Kind, Realm, Workspace};
use serde_json::json;

#[test]
fn workspace_origins_are_canonical_and_reject_untrusted_destinations() {
    for input in ["https://Team-9.slack.com", "https://team-9.slack.com:443/"] {
        let workspace = Workspace::try_from(input.to_owned()).unwrap();
        assert_eq!(workspace.to_string(), "https://team-9.slack.com/");
        assert_eq!(workspace.api_url(), "https://team-9.slack.com/api");
    }
    for input in [
        "http://team.slack.com",
        "https://slack.com.evil.example",
        "https://team.slack.com.evil.example",
        "https://slack.com",
        "https://team.sub.slack.com",
        "https://team.slack.com.",
        "https://team.slack.com:444",
        "https://user:password@team.slack.com",
        "https://@team.slack.com",
        "https://team.slack.com/api",
        "https://team.slack.com/?token=hidden",
        "https://team.slack.com/#fragment",
        "https://127.0.0.1",
        "https://[::1]",
        "https://team.slack-gov.com.evil.example",
        "https://-team.slack.com",
        " https://team.slack.com",
        "https://team.slack.com\n",
        "https://team.slack.com\\",
    ] {
        let error = Workspace::try_from(input.to_owned()).unwrap_err();
        assert_eq!(error.report().code, "invalid_workspace", "{input}");
        assert!(!error.to_string().contains(input));
    }
}

#[test]
fn credential_kinds_roundtrip_without_exposing_secrets_in_debug() {
    let cases = [
        (Kind::User, json!({"token": "xoxp-synthetic-user"})),
        (
            Kind::Browser,
            json!({
                "workspace_url": "https://team.slack.com/",
                "token": "xoxc-synthetic-browser",
                "cookie": "xoxd-synthetic%2Bcookie%3D"
            }),
        ),
        (
            Kind::User,
            json!({
                "token": "xoxe.xoxp-synthetic",
                "rotation": {
                    "client_id": "1234.5678", "refresh_token": "xoxe-synthetic",
                    "expires_at": 1900000000, "scope": "chat:write"
                }
            }),
        ),
    ];
    for (kind, input) in cases {
        let credentials = Credentials::parse(kind, &input.to_string()).unwrap();
        let debug = format!("{credentials:?}");
        assert!(!debug.contains(input["token"].as_str().unwrap()));
        if let Some(cookie) = input["cookie"].as_str() {
            assert!(!debug.contains(cookie));
        }
        if let Some(refresh) = input["rotation"]["refresh_token"].as_str() {
            assert!(!debug.contains(refresh));
        }
        assert_eq!(credentials.kind(), kind);
        let stored = credentials.encode_for_keyring().unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&stored).unwrap(),
            input
        );
        let restored = Credentials::parse(kind, &stored).unwrap();
        assert_eq!(*restored.encode_for_keyring().unwrap(), *stored);
    }
}

#[test]
fn credential_import_rejects_wrong_kinds_header_injection_and_unknown_fields() {
    for (kind, input) in [
        (Kind::User, json!({"token": "xoxb-synthetic-bot"})),
        (Kind::User, json!({"token": "xoxc-synthetic-browser"})),
        (Kind::User, json!({"token": "xoxp-"})),
        (Kind::User, json!({"token": "xoxp-secret\r\nInjected: yes"})),
        (
            Kind::User,
            json!({"token": "xoxp-user", "cookie": "xoxd-secret"}),
        ),
        (
            Kind::User,
            json!({"token": "xoxp-user", "unknown": "xoxp-secret"}),
        ),
        (Kind::Browser, json!({"token": "xoxc-browser"})),
        (
            Kind::Browser,
            json!({"token": "xoxc-browser", "cookie": "xoxd-secret"}),
        ),
        (
            Kind::Browser,
            json!({"token": "xoxc-browser", "cookie": "xoxd-secret; other=bad", "workspace_url": "https://team.slack.com"}),
        ),
        (
            Kind::Browser,
            json!({"token": "xoxc-browser", "cookie": "xoxd-secret\r\nInjected: yes", "workspace_url": "https://team.slack.com"}),
        ),
        (
            Kind::Browser,
            json!({"token": "xoxc-browser", "cookie": "xoxd-secret", "workspace_url": "https://evil.example"}),
        ),
    ] {
        let error = Credentials::parse(kind, &input.to_string()).unwrap_err();
        assert_eq!(error.report().code, "invalid_credentials");
        assert!(!format!("{error:?} {error}").contains("xox"));
    }
    assert!(
        Credentials::parse(
            Kind::User,
            r#"{"token":"xoxp-first","token":"xoxp-second"}"#
        )
        .is_err()
    );
}

#[test]
fn gov_credentials_bind_the_realm_without_accepting_other_hosts_or_kinds() {
    let workspace = Workspace::try_from("https://Agency-9.slack-gov.com:443".to_owned()).unwrap();
    assert_eq!(workspace.realm(), Realm::Gov);
    assert_eq!(workspace.to_string(), "https://agency-9.slack-gov.com/");
    assert_eq!(Realm::Gov.api_url(), "https://slack-gov.com/api");
    assert_eq!(
        Realm::Gov.authorization_url(),
        "https://slack-gov.com/oauth/v2/authorize"
    );
    for (kind, token) in [(Kind::User, "xoxp-gov-test"), (Kind::Bot, "xoxb-gov-test")] {
        let input = json!({"token": token, "realm": "gov"});
        let credentials = Credentials::parse(kind, &input.to_string()).unwrap();
        assert_eq!(credentials.kind(), kind);
        assert_eq!(credentials.realm(), Realm::Gov);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&credentials.encode_for_keyring().unwrap())
                .unwrap(),
            input
        );
    }
    for input in [
        json!({"token": "xoxc-gov-test", "cookie": "xoxd-gov-test", "workspace_url": workspace, "realm": "commercial"}),
        json!({"token": "xoxc-gov-test", "cookie": "xoxd-gov-test", "workspace_url": "https://team.slack.com", "realm": "gov"}),
    ] {
        assert!(Credentials::parse(Kind::Browser, &input.to_string()).is_err());
    }
    assert!(Credentials::parse(Kind::Bot, r#"{"token":"xoxp-not-a-bot"}"#).is_err());
    assert!(Credentials::parse(Kind::Bot, r#"{"token":"xoxb-bot","cookie":"xoxd-test"}"#).is_err());
}
