use super::{Backend, Credentials, Identity, Kind, Rotation, Store, Workspace};
use crate::error::Error;
use serde_json::json;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::{PermissionsExt, symlink};

fn identity() -> Identity {
    Identity {
        workspace_url: Workspace::try_from("https://team.slack.com/".to_owned()).unwrap(),
        team_id: "T12345678".to_owned(),
        user_id: "U87654321".to_owned(),
        bot_id: None,
    }
}

fn profile(team: &str, user: &str) -> serde_json::Value {
    json!({
        "kind": "user",
        "identity": {"workspace_url": "https://team.slack.com/", "team_id": team, "user_id": user},
        "keyring_ref": "00112233-4455-4677-8899-aabbccddeeff"
    })
}

#[test]
fn absent_configuration_is_read_only_and_has_no_selected_identity() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("slk");
    let store = Store::load(directory.clone()).unwrap();
    assert!(store.profiles().is_empty());
    assert!(matches!(store.select(None), Err(Error::NoProfiles)));
    assert_eq!(store.settings().request_timeout_secs.get(), 30);
    assert_eq!(store.settings().login_timeout_secs.get(), 300);
    assert_eq!(store.settings().browser.executable, None);
    assert!(!directory.exists());
}

#[test]
fn stored_profiles_require_explicit_selection_when_ambiguous_and_tighten_permissions() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("config.json");
    fs::write(
        &path,
        json!({"settings": {"request_timeout_secs": 7, "login_timeout_secs": 41, "browser": {"executable": "/custom/chrome"}}, "profiles": {
            "work": profile("T1234", "U1234"),
            "personal": profile("T4567", "W4567")
        }})
        .to_string(),
    )
    .unwrap();
    #[cfg(unix)]
    {
        fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    }

    let store = Store::load(temporary.path().to_owned()).unwrap();
    assert!(matches!(store.select(None), Err(Error::AmbiguousProfile)));
    assert!(matches!(
        store.select(Some("missing")),
        Err(Error::ProfileNotFound)
    ));
    assert_eq!(
        store.select(Some("work")).unwrap().identity.user_id,
        "U1234"
    );
    assert_eq!(
        store.select(Some("personal")).unwrap().identity.user_id,
        "W4567"
    );
    assert_eq!(store.settings().request_timeout_secs.get(), 7);
    assert_eq!(store.settings().login_timeout_secs.get(), 41);
    assert_eq!(
        store.settings().browser.executable.as_deref(),
        Some(std::path::Path::new("/custom/chrome"))
    );
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(temporary.path()).unwrap().permissions().mode() & 0o777,
        0o700
    );
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let output = serde_json::to_string(&store.profiles()).unwrap();
    assert!(!output.contains("keyring_ref"));
}

#[test]
fn authentication_lock_excludes_mutations_reloads_metadata_and_releases_on_drop() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("slk");
    let mut first = Store::load(directory.clone()).unwrap();
    let mut second = Store::load(directory.clone()).unwrap();
    let lock = first.lock().unwrap();
    assert_eq!(
        second.lock().unwrap_err().report().code,
        "authentication_busy"
    );
    assert!(
        Store::load(directory.clone())
            .unwrap()
            .profiles()
            .is_empty()
    );
    fs::write(
        directory.join("config.json"),
        json!({
            "profiles": {"new": profile("T1234", "U5678")}
        })
        .to_string(),
    )
    .unwrap();
    assert!(second.profiles().is_empty());
    drop(lock);
    let _lock = second.lock().unwrap();
    assert_eq!(
        second.select(Some("new")).unwrap().identity.user_id,
        "U5678"
    );
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(directory.join("auth.lock"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn invalid_configuration_and_symlinks_are_not_replaced() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("config.json");
    for contents in [
        "not JSON".to_owned(),
        json!({"settings": {"login_timeout_secs": 0}}).to_string(),
        json!({"settings": {"browser": {"profile": "/daily/browser"}}}).to_string(),
        json!({"settings": {"request_timeout_secs": 0}}).to_string(),
        json!({"token": "xoxp-should-not-be-here"}).to_string(),
        json!({"profiles": {"work": {"kind": "user", "identity": {"workspace_url": "http://team.slack.com", "team_id": "T123", "user_id": "U123"}, "keyring_ref": "00112233-4455-4677-8899-aabbccddeeff"}}}).to_string(),
    ] {
        fs::write(&path, &contents).unwrap();
        assert!(matches!(Store::load(temporary.path().to_owned()), Err(Error::Config)));
        assert_eq!(fs::read_to_string(&path).unwrap(), contents);
    }
    fs::remove_file(&path).unwrap();
    #[cfg(unix)]
    {
        let target = temporary.path().join("other.json");
        fs::write(&target, "{}").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
        symlink(&target, &path).unwrap();
        assert!(matches!(
            Store::load(temporary.path().to_owned()),
            Err(Error::ConfigPath)
        ));
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }
}

#[test]
fn existing_profile_cannot_silently_switch_identity_or_kind() {
    let temporary = tempfile::tempdir().unwrap();
    let original = json!({"profiles": {"work": profile("T12345678", "U87654321")}}).to_string();
    fs::write(temporary.path().join("config.json"), &original).unwrap();
    let mut store = Store::load(temporary.path().to_owned()).unwrap();
    let user = Credentials::parse(Kind::User, r#"{"token":"xoxp-synthetic"}"#).unwrap();
    let mut other = identity();
    other.user_id = "U11111111".to_owned();
    assert!(matches!(
        store.login("work", &user, other),
        Err(Error::ProfileConflict)
    ));
    let browser = Credentials::parse(Kind::Browser, r#"{"token":"xoxc-synthetic","cookie":"xoxd-synthetic","workspace_url":"https://team.slack.com"}"#).unwrap();
    assert!(matches!(
        store.login("work", &browser, identity()),
        Err(Error::ProfileConflict)
    ));
    assert_eq!(
        fs::read_to_string(temporary.path().join("config.json")).unwrap(),
        original
    );
}

#[test]
#[ignore = "Requires an unlocked native Keychain or Secret Service; uses only synthetic credentials"]
fn native_keyring_roundtrip_refresh_and_logout() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("slk");
    let mut store = Store::load(directory.clone()).unwrap();
    let original =
        Credentials::parse(Kind::User, r#"{"token":"xoxp-synthetic-original"}"#).unwrap();
    let refreshed =
        Credentials::parse(Kind::User, r#"{"token":"xoxp-synthetic-refreshed"}"#).unwrap();
    store.login("test", &original, identity()).unwrap();
    let Backend::Persistent { config, .. } = &store.backend else {
        unreachable!()
    };
    let entry = config.profiles["test"].entry().unwrap();
    let outcome = (|| {
        let mut reopened = Store::load(directory.clone())?;
        assert_eq!(reopened.select(None)?.identity, identity());
        assert_eq!(
            *reopened.credentials("test")?.encode_for_keyring()?,
            *original.encode_for_keyring()?
        );
        let refreshed = refreshed.with_rotation(Some(Rotation {
            client_id: "1234.5678".to_owned(),
            refresh_token: "xoxe-synthetic-refresh".into(),
            expires_at: 1900000000,
            scope: "chat:write".to_owned(),
        }))?;
        reopened.login("test", &refreshed, identity())?;
        assert_eq!(
            *reopened.credentials("test")?.encode_for_keyring()?,
            *refreshed.encode_for_keyring()?
        );
        let metadata = fs::read_to_string(directory.join("config.json")).unwrap();
        assert!(!metadata.contains("xoxp-"));
        assert!(!metadata.contains("xoxe-"));
        assert!(!metadata.contains("rotation"));
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(directory.join("config.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        entry
            .set_password("malformed synthetic credential")
            .unwrap();
        let error = reopened.credentials("test").unwrap_err();
        assert_eq!(error.report().code, "invalid_stored_credential");
        assert_eq!(error.exit_code(), 1);
        reopened.login("test", &refreshed, identity())?;
        reopened.logout("test")?;
        assert!(Store::load(directory.clone())?.profiles().is_empty());
        Ok::<(), Error>(())
    })();
    let remaining = entry.get_password();
    if remaining.is_ok() {
        entry.delete_credential().unwrap();
    }
    outcome.unwrap();
    assert!(matches!(remaining, Err(keyring_core::Error::NoEntry)));
}

#[test]
#[ignore = "Requires an unlocked native Keychain or Secret Service; uses only synthetic credentials"]
fn native_keyring_browser_login_preserves_identity_and_rejects_failed_persistence() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("slk");
    let mut store = Store::load(directory.clone()).unwrap();
    let credentials = Credentials::parse(
        Kind::Browser,
        r#"{"token":"xoxc-synthetic","cookie":"xoxd-synthetic%2Bvalue","workspace_url":"https://team.slack.com"}"#,
    )
    .unwrap();
    fs::write(&directory, "unrelated existing file").unwrap();
    assert!(matches!(
        store.login("browser", &credentials, identity()),
        Err(Error::ConfigPath)
    ));
    assert!(store.profiles().is_empty());
    assert_eq!(
        fs::read_to_string(&directory).unwrap(),
        "unrelated existing file"
    );

    fs::remove_file(&directory).unwrap();
    store.login("browser", &credentials, identity()).unwrap();
    let Backend::Persistent { config, .. } = &store.backend else {
        unreachable!()
    };
    let entry = config.profiles["browser"].entry().unwrap();
    let outcome = (|| {
        let mut reopened = Store::load(directory.clone())?;
        assert_eq!(
            *reopened.credentials("browser")?.encode_for_keyring()?,
            *credentials.encode_for_keyring()?
        );
        let mut other = identity();
        other.user_id = "U11111111".to_owned();
        assert!(matches!(
            reopened.login("browser", &credentials, other),
            Err(Error::ProfileConflict)
        ));
        assert_eq!(reopened.select(Some("browser"))?.identity, identity());
        assert!(
            !fs::read_to_string(directory.join("config.json"))
                .unwrap()
                .contains("xox")
        );
        reopened.logout("browser")?;
        Ok::<(), Error>(())
    })();
    let remaining = entry.get_password();
    if remaining.is_ok() {
        entry.delete_credential().unwrap();
    }
    outcome.unwrap();
    assert!(matches!(remaining, Err(keyring_core::Error::NoEntry)));
}
