use super::{MessageLink, MessageText};
use crate::auth::Workspace;
use crate::ids::{parse_channel, parse_timestamp};

#[test]
fn message_links_preserve_microseconds_and_enforce_workspace_and_root_hints() {
    let link: MessageLink = "https://team.slack.com/archives/C1234/p1700000000000002?thread_ts=1700000000.000001&cid=C1234".parse().unwrap();
    assert_eq!(link.ts.0, "1700000000.000002");
    assert_eq!(link.root_hint(None).unwrap().0, "1700000000.000001");
    assert_eq!(link.channel_id.0, "C1234");
    let workspace = Workspace::try_from("https://team.slack.com".to_owned()).unwrap();
    link.check_workspace(&workspace).unwrap();
    let other = Workspace::try_from("https://other.slack.com".to_owned()).unwrap();
    assert_eq!(
        link.check_workspace(&other).unwrap_err().report().code,
        "workspace_mismatch"
    );
    assert!(
        link.root_hint(Some(&parse_timestamp("1700000000.000003").unwrap()))
            .is_err()
    );
    let bare: MessageLink = "https://team.slack.com/archives/D1234/p1700000000000001"
        .parse()
        .unwrap();
    assert_eq!(bare.root_hint(None).unwrap().0, "1700000000.000001");
}

#[test]
fn invalid_message_targets_are_rejected_without_url_normalization_bypasses() {
    for value in [
        "http://team.slack.com/archives/C1234/p1700000000000001",
        "https://team.slack.com.evil.example/archives/C1234/p1700000000000001",
        "https://team.slack.com:8443/archives/C1234/p1700000000000001",
        "https://user:password@team.slack.com/archives/C1234/p1700000000000001",
        "https://team.slack.com/archives/U1234/p1700000000000001",
        "https://team.slack.com/archives/C1234/p1700000000000001/extra",
        "https://team.slack.com/archives/C1234/p1700000000.000001",
        "https://team.slack.com/archives/C1234/p1700000000000001?thread_ts=1.000001&thread_ts=2.000001",
        "https://team.slack.com/archives/C1234/p1700000000000001?cid=C9999",
        "https://team.slack.com/archives/C1234/p1700000000000001?thread_ts=1e3",
        "https://team.slack.com/archives/C1234/p000001",
        "https://team.slack.com/archives/C1234/p1700000000000001\n",
        "https://team.slack.com\\archives\\C1234\\p1700000000000001",
    ] {
        assert!(value.parse::<MessageLink>().is_err(), "{value}");
    }
    for id in ["general", "#general", "U1234", "C", "C1234\n", "c1234"] {
        assert!(parse_channel(id).is_err());
    }
    for ts in [
        "1700000000",
        "1700000000.1",
        "1.0000017",
        "1e3.000001",
        "-1.000001",
        "1.00a001",
    ] {
        assert!(parse_timestamp(ts).is_err());
    }
}

#[test]
fn single_message_limit_counts_unicode_characters_and_preserves_the_body() {
    let text = "🦀".repeat(4_000);
    assert_eq!(MessageText::try_from(text.clone()).unwrap().as_ref(), text);
    assert_eq!(
        MessageText::try_from("🦀".repeat(4_001))
            .unwrap_err()
            .report()
            .code,
        "message_too_long"
    );
    assert!(MessageText::try_from(" \n\t".to_owned()).is_err());
    let literal = "  $(echo never-execute)\ntext\r\n";
    assert_eq!(
        MessageText::try_from(literal.to_owned()).unwrap().as_ref(),
        literal
    );
}
