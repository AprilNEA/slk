use super::{Output, terminal_text};
use crate::auth::{Identity, Kind, ProfileView, Workspace};
use crate::messages::{Change, Message, MessageText};
use crate::slack::{Channel, Page, Pagination, Resolution, ResolutionStatus};
use crate::users::User;
use serde_json::{Value, json};
use slack_morphism::SlackConversationType;

#[test]
fn terminal_rendering_removes_sequences_and_overwrite_controls_but_preserves_text() {
    let untrusted =
        "before\x1b]52;c;c2VjcmV0\x07after\x1b[2J\x1b[H\r覆盖\x08!\0\x7f\u{0085}\nnext\tcell";
    assert_eq!(terminal_text(untrusted), "beforeafter覆盖!\nnext\tcell");
    assert_eq!(terminal_text("a\x1b]0;changed title\x1b\\b"), "ab");
    assert_eq!(terminal_text("a\x1bPdiscard\x1b\\b"), "ab");
    assert_eq!(terminal_text("valid 🦀 e\u{301}"), "valid 🦀 e\u{301}");
    assert_eq!(terminal_text("visible\x1b]52;c;unterminated"), "visible");
}

#[test]
fn message_output_isolates_fields_and_preserves_later_messages_and_pagination() {
    for (next_cursor, notice) in [
        (Some("next+/="), "Next cursor: next+/="),
        (None, "Slack reports more results but provided no cursor."),
    ] {
        for suffix in ["\x1b]0;discarded title", "\x1bP1;2|discarded data"] {
            for field in [
                "channel_id",
                "ts",
                "thread_ts",
                "author",
                "text",
                "permalink",
            ] {
                let mut first = Message {
                    channel_id: "C1".into(),
                    ts: "1700000000.000002".into(),
                    thread_ts: Some("1700000000.000001".into()),
                    author: Some("U1".into()),
                    text: "first message 🦀\nsecond line".into(),
                    blocks: None,
                    permalink: Some("https://team.slack.com/archives/C1/p1700000000000002".into()),
                };
                let value = match field {
                    "channel_id" => &mut first.channel_id.0,
                    "ts" => &mut first.ts.0,
                    "thread_ts" => &mut first.thread_ts.as_mut().unwrap().0,
                    "author" => first.author.as_mut().unwrap(),
                    "text" => &mut first.text,
                    "permalink" => first.permalink.as_mut().unwrap(),
                    _ => unreachable!(),
                };
                value.push_str(suffix);
                let original = value.clone();
                let output = Output::Messages(Page {
                    data: vec![
                        first,
                        Message {
                            channel_id: "C9".into(),
                            ts: "1700000001.000003".into(),
                            thread_ts: None,
                            author: Some("B9".into()),
                            text: "later message".into(),
                            blocks: None,
                            permalink: None,
                        },
                    ],
                    pagination: Pagination::Cursor {
                        next_cursor: next_cursor.map(str::to_owned),
                        has_more: Some(true),
                    },
                });
                let mut text = Vec::new();
                output.write(false, &mut text).unwrap();
                let text = String::from_utf8(text).unwrap();
                for expected in [
                    "C1",
                    "1700000000.000002",
                    "1700000000.000001",
                    "U1",
                    "first message 🦀\nsecond line",
                    "https://team.slack.com/archives/C1/p1700000000000002",
                    "C9",
                    "1700000001.000003",
                    "B9",
                    "later message",
                    notice,
                ] {
                    assert!(
                        text.contains(expected),
                        "{field}: missing {expected:?} in {text:?}"
                    );
                }
                assert!(!text.contains('\x1b'));
                assert!(!text.contains("discarded"));

                let mut encoded = Vec::new();
                output.write(true, &mut encoded).unwrap();
                let decoded: Value = serde_json::from_slice(&encoded).unwrap();
                assert_eq!(decoded["data"].as_array().unwrap().len(), 2);
                assert_eq!(decoded["data"][0][field], original);
                assert_eq!(decoded["data"][1]["text"], "later message");
                assert_eq!(
                    decoded["pagination"],
                    json!({"next_cursor": next_cursor, "has_more": true})
                );
            }
        }
    }
}

#[test]
fn channel_output_preserves_archive_status_and_continuation_after_untrusted_fields() {
    let name = "decommissioned\x1b]0;discarded title";
    let user = "U9\x1bP1;2|discarded data";
    let cursor = "later+/=\x1b]0;discarded cursor";
    let output = Output::Channels(Page {
        data: vec![
            Channel {
                channel_id: "C1".into(),
                name: Some(name.into()),
                conversation_type: Some(SlackConversationType::Public),
                user_id: None,
                is_archived: true,
            },
            Channel {
                channel_id: "D2".into(),
                name: None,
                conversation_type: Some(SlackConversationType::Im),
                user_id: Some(user.into()),
                is_archived: false,
            },
        ],
        pagination: Pagination::Cursor {
            next_cursor: Some(cursor.into()),
            has_more: None,
        },
    });
    let mut text = Vec::new();
    output.write(false, &mut text).unwrap();
    let text = String::from_utf8(text).unwrap();
    for expected in [
        "C1",
        "decommissioned",
        "(archived)",
        "D2",
        "U9",
        "Next cursor: later+/=",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in {text:?}");
    }
    assert!(!text.contains('\x1b'));
    assert!(!text.contains("discarded"));

    let mut encoded = Vec::new();
    output.write(true, &mut encoded).unwrap();
    let decoded: Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded["data"][0]["name"], name);
    assert_eq!(decoded["data"][1]["user_id"], user);
    assert_eq!(decoded["pagination"]["next_cursor"], cursor);
}

#[test]
fn user_output_keeps_account_flags_candidates_and_paging_after_untrusted_fields() {
    let original = json!({
        "id": "U1", "name": "alice\u{001b}]0;discarded name", "real_name": "Alice Smith\u{001b}Pdiscarded real name",
        "deleted": true, "is_app_user": true,
        "profile": {"display_name": "Ali\u{001b}]0;discarded display name", "email": "alice@example.test\u{001b}Pdiscarded email"}
    });
    for resolved in [false, true] {
        let page = Page {
            data: vec![
                serde_json::from_value::<User>(original.clone()).unwrap(),
                serde_json::from_value::<User>(json!({"id": "U9", "name": "later user"})).unwrap(),
            ],
            pagination: Pagination::Cursor {
                next_cursor: Some("next+/=".into()),
                has_more: Some(true),
            },
        };
        let output = if resolved {
            Output::ResolvedUsers(Resolution {
                status: ResolutionStatus::Ambiguous,
                candidates: page,
            })
        } else {
            Output::Users(page)
        };
        let mut text = Vec::new();
        output.write(false, &mut text).unwrap();
        let text = String::from_utf8(text).unwrap();
        for expected in [
            "U1",
            "alice",
            "Ali",
            "Alice Smith",
            "alice@example.test",
            "(deactivated)",
            "(app)",
            "U9",
            "later user",
            "Next cursor: next+/=",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in {text:?}");
        }
        if resolved {
            assert!(text.contains("Ambiguous name"));
            assert!(text.contains("no target was selected"));
        }
        assert!(!text.contains('\x1b'));
        assert!(!text.contains("discarded"));
        let mut encoded = Vec::new();
        output.write(true, &mut encoded).unwrap();
        let decoded: Value = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded["data"][0]["name"], original["name"]);
        assert_eq!(
            decoded["data"][0]["profile"]["email"],
            original["profile"]["email"]
        );
        assert_eq!(decoded["data"][0]["deleted"], true);
        assert_eq!(decoded["data"][1]["user_id"], "U9");
        assert_eq!(decoded["pagination"]["next_cursor"], "next+/=");
    }
}

#[test]
fn write_previews_preserve_actions_and_targets_after_unterminated_sequences() {
    for action in ["dm", "edit", "delete", "react", "unreact"] {
        let profile = ProfileView {
            profile: "work".into(),
            kind: Kind::User,
            identity: Identity {
                workspace_url: Workspace::try_from("https://team.slack.com/".to_owned()).unwrap(),
                team_id: "T1".into(),
                user_id: "U1".into(),
                bot_id: None,
            },
        };
        let message = Message {
            channel_id: "C1".into(),
            ts: "1700000000.000002".into(),
            thread_ts: Some("1700000000.000001".into()),
            author: Some("U1".into()),
            text: "original\x1b]0;discarded title".into(),
            blocks: None,
            permalink: None,
        };
        let output = if action == "dm" {
            Output::DmPreview {
                dry_run: true,
                profile,
                target: serde_json::from_value(
                    json!({"id": "U7", "name": "Alice\u{001b}]0;discarded name"}),
                )
                .unwrap(),
            }
        } else {
            let change = match action {
                "edit" => Change::Edit {
                    content: MessageText::try_from("new text\x1bPdiscarded data".to_owned())
                        .unwrap()
                        .into(),
                },
                "delete" => Change::Delete,
                "react" => Change::React {
                    name: "wave".into(),
                },
                "unreact" => Change::Unreact {
                    name: "wave".into(),
                },
                _ => unreachable!(),
            };
            Output::ChangePreview {
                dry_run: true,
                profile,
                message,
                change,
            }
        };
        let mut text = Vec::new();
        output.write(false, &mut text).unwrap();
        let text = String::from_utf8(text).unwrap();
        for expected in [
            "Dry run",
            "Profile: work",
            "Workspace: https://team.slack.com/ (T1)",
            "User: U1",
            "Action:",
        ] {
            assert!(
                text.contains(expected),
                "{action}: missing {expected:?} in {text:?}"
            );
        }
        if action == "dm" {
            assert!(text.contains("U7\tAlice"));
            assert!(text.contains("No message will be sent."));
        } else {
            assert!(text.contains("1700000000.000002"));
            assert!(text.contains("original"));
            if action == "edit" {
                assert!(text.contains("New text:\nnew text"));
            }
        }
        assert!(!text.contains('\x1b'));
        assert!(!text.contains("discarded"));
        let mut encoded = Vec::new();
        output.write(true, &mut encoded).unwrap();
        let decoded: Value = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded["dry_run"], true);
        if action == "dm" {
            assert_eq!(decoded["target"]["name"], "Alice\x1b]0;discarded name");
        } else {
            assert_eq!(decoded["action"], action);
            assert_eq!(decoded["message"]["text"], "original\x1b]0;discarded title");
        }
    }
}
