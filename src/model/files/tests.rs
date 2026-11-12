use super::{File, Upload};
use crate::auth::FileSettings;
use serde_json::json;
use std::fs;

#[test]
fn file_urls_reject_remote_files_and_untrusted_destinations_and_remain_private() {
    for url in [
        "http://files.slack.com/file",
        "https://files.slack.com.evil.example/file",
        "https://team.slack.com/file",
        "https://files.slack.com:8443/file",
        "https://name:secret@files.slack.com/file",
        "https://127.0.0.1/file",
    ] {
        let file: File =
            serde_json::from_value(json!({"id": "F1", "url_private": url, "future": true}))
                .unwrap();
        assert_eq!(
            file.download_url().err().unwrap().report().code,
            "unsafe_file_url"
        );
        assert!(!serde_json::to_string(&file).unwrap().contains(url));
    }
    for flag in [
        json!({"is_external": true}),
        json!({"mode": "external"}),
        json!({"filetype": "remote"}),
    ] {
        let mut file = json!({"id": "F1", "url_private": "https://files.slack.com/remote"});
        file.as_object_mut()
            .unwrap()
            .extend(flag.as_object().unwrap().clone());
        let file: File = serde_json::from_value(file).unwrap();
        assert_eq!(
            file.download_url().err().unwrap().report().code,
            "external_file"
        );
    }
    let file: File = serde_json::from_value(
        json!({"id": "F1", "url_private": "https://files.slack.com/original"}),
    )
    .unwrap();
    assert!(file.download_url().is_ok());
}

#[test]
fn upload_snapshots_exact_bytes_and_enforces_the_configured_boundary() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("a+é.txt");
    let settings = FileSettings {
        max_bytes: 8.try_into().unwrap(),
        ..FileSettings::default()
    };
    fs::write(&path, b"one\0two\n").unwrap();
    let upload = Upload::read(&path, &settings).unwrap();
    fs::write(&path, b"ninebytes").unwrap();
    assert_eq!(upload.name, "a+é.txt");
    assert_eq!(upload.content.as_ref(), b"one\0two\n");
    assert_eq!(
        Upload::read(&path, &settings).err().unwrap().report().code,
        "file_too_large"
    );
    fs::write(&path, b"").unwrap();
    assert_eq!(
        Upload::read(&path, &settings).err().unwrap().report().code,
        "invalid_arguments"
    );
    assert!(Upload::read(home.path(), &settings).is_err());
}
