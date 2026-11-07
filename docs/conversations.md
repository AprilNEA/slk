# Conversations and workspace information

These commands verify the selected profile with `auth.test`. Reading never joins or opens a conversation, changes a read cursor, or silently obtains additional scopes. Writes follow the same [preview and approval rules](messages.md#preview-and-send) as messages. A scope is a prerequisite, not permission to bypass workspace policies or membership restrictions.

## Inspect conversations

```sh
slk channel info C0123456789 --profile work --json
slk channel members G0123456789 --profile work --limit 20 --json
slk group-dm list --profile work --limit 20 --json
slk unread --profile work --limit 20 --json
slk workspace --profile work --json
```

`channel info` accepts an explicit conversation ID, including a DM or group DM. It returns the ID, name, type, archive status, optional membership and topic, and read-state fields that Slack actually supplies. `channel members` returns one page of member IDs by default; continue with `--cursor` or explicitly traverse with `--all --max-pages N`. `group-dm list` reads the selected identity's existing MPIM conversations through `users.conversations`, without calling `conversations.open`; it uses the same bounded pagination options.

`unread` returns one page of joined conversations and their available `last_read`, `unread_count`, and `unread_count_display` by default; `--all` follows continuations within the page budget. It does not filter out zero or unknown counts and makes no additional per-conversation requests. Missing fields remain omitted in JSON and appear as `unknown` in text, never as zero. Slack does not promise counts for every conversation type. This is not an implementation of Slack's All unreads, Threads unread state, or client badge totals. Ordinary `read`, `thread`, and `unread` calls never mark messages read.

`workspace` returns the selected workspace's `team.info` response after checking its team ID. It provides read-only information, not organization settings, membership administration, SCIM, or Enterprise management.

## Change a channel explicitly

```sh
slk channel create release-planning --visibility private --profile work --dry-run
slk channel join C0123456789 --profile work --dry-run
slk channel topic C0123456789 'Release planning and status' --profile work --dry-run
slk channel leave C0123456789 --profile work --dry-run
slk channel archive C0123456789 --profile work --dry-run
slk channel mark C0123456789 1700000000.000007 --profile work --dry-run
```

All examples preview without making a mutation request. Actual non-interactive or JSON writes require both `--profile` and `--yes`. An interactive write previews the identity, current conversation, and requested action, then defaults to cancellation. No operation sends a message implicitly.

- `create` requires `--visibility public` or `--visibility private`. Names must contain 1–80 lowercase ASCII letters, digits, underscores, or hyphens. slk does not silently normalize names or assume public visibility. Creation has no automatic rollback.
- `join` accepts public channels only. It does not enter an uninvited private channel.
- `topic` replaces the plain-text topic, up to 250 Unicode scalar values; an empty string clears it. It does not change the channel purpose or description.
- `leave` affects the selected user's membership. Leaving a private channel can require another member's invitation to rejoin.
- `archive` affects the channel for its members but does not delete history. slk does not automatically unarchive or compensate for an uncertain result.
- `mark` explicitly sets the selected user's Slack read cursor to the supplied timestamp across clients. It is not a promise of client-level Mark unread or thread unread behavior. Timestamps remain exact strings with six fractional digits.

Except for creation, each mutation first inspects the specified conversation and checks the command's conversation type. Slack applies membership, role, required-channel, guest, and workspace policy restrictions. A preview cannot freeze membership or prevent another client from changing the channel.

## Open, resume, or close a group DM

```sh
slk group-dm open U0123456789 W9876543210 --profile work --dry-run
slk group-dm resume G0123456789 --profile work --dry-run
slk group-dm close G0123456789 --profile work --dry-run
```

`open` accepts 2–8 distinct other user IDs, not the selected user. Slack automatically includes the caller. slk fetches each user's details, rejects deactivated accounts and apps, and previews the requested participants before calling `conversations.open`. The same participant set can return an existing conversation. Cross-workspace visibility does not guarantee that Slack permits a new group DM.

Success includes the returned `channel`, `requested_users`, and optional `already_open` and `no_op` flags. `requested_users` records the submitted request, not a separately verified membership list. Use `channel members` when you need to inspect actual membership. No message is sent and no additional member-list request occurs automatically.

`resume` and `close` require an existing MPIM conversation ID. Both are writes, even when resuming an existing conversation or hiding it only for the selected user. Closing does not delete history or remove other members. These commands do not change an existing group's participants or convert it to a channel.

## Permissions and failure outcomes

| Operation | Relevant user scopes |
| --- | --- |
| Inspect or list conversations and members | The applicable `channels:read`, `groups:read`, `im:read`, or `mpim:read` |
| Create, leave, or archive a channel | `channels:write` for public channels; `groups:write` for private channels |
| Join a public channel | `channels:write` |
| Set a topic | `channels:write.topic` or `groups:write.topic`; Slack also accepts the corresponding broader write scope |
| Open, resume, or close a group DM | `mpim:write`; opening also requires `users:read` for participant inspection |
| Set a read cursor | The applicable `channels:write`, `groups:write`, `im:write`, or `mpim:write` |
| Read workspace information | `team:read` |

Mutations that inspect a conversation also need its corresponding read scope. slk never requests every scope automatically or upgrades to an administrative API after rejection. Read-cursor write scopes permit more than the command itself exposes; authorize only the modules you need.

All requests are single-attempt. Lost acknowledgements, timeout, HTTP 5xx, ambiguous Slack internal errors, or invalid success responses produce `write_outcome_unknown`; inspect Slack before repeating an operation. Explicit permission errors remain errors, not empty lists or successful no-ops. These contracts are tested against local fixtures; actual workspace permissions and client behavior still need authorized live acceptance.
