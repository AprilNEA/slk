# Users, target resolution, and direct messages

Every command verifies the selected profile with `auth.test`. Reads and dry-runs can select the only configured profile implicitly. Multiple profiles require `--profile`; non-interactive or JSON writes always require an explicit profile. These commands do not switch identities, retry requests, or store a user directory locally.

## Query users

```sh
slk users list --profile work --limit 20 --json
slk users info U0123456789 --profile work --json
slk users lookup alice@example.com --profile work --json
```

`users list` fetches one page through `users.list` by default. Continue with `--cursor` from `pagination.next_cursor`, or explicitly use `--all --max-pages N`; a short page is not proof of completion. The per-page limits, page budgets, and cursor rules match [conversation pagination](messages.md#pagination).

`users info` accepts a user ID beginning with `U` or `W`, not a display name, email, or conversation ID. `users lookup` calls `users.lookupByEmail` and requires the corresponding email grant. Missing scopes, inaccessible users, and deactivated-user lookup failures retain Slack's error codes rather than returning empty success results.

List results use the `data` and `pagination` envelope. Info and lookup return a single object with `user_id`, `team_id`, `name`, `real_name`, `profile`, `deleted`, `is_bot`, and `is_app_user`. Optional fields can be null; `profile`, when present, contains `display_name`, `real_name`, and `email`. Unknown response fields are ignored. Lists include deactivated accounts and app users rather than hiding their status.

## Resolve names explicitly

```sh
slk resolve user '@Alice' --profile work --json
slk resolve channel '#general' --profile work --json
```

Resolution matches exact usernames, real names, display names, or channel names. Matching ignores ASCII letter case and accepts an optional leading `@` for users or `#` for channels. It does not perform fuzzy matching, expand mentions, select the first candidate, or modify `send` behavior. Choose an explicit ID for the next command.

Resolution reads one page by default, or the bounded page set selected by `--all`, and returns `status`, `data`, and `pagination`:

| Status | Meaning |
| --- | --- |
| `unique` | One matching candidate, no input cursor, and no reported continuation |
| `not_found` | No matching candidate, no input cursor, and no reported continuation |
| `ambiguous` | At least two matching candidates across the fetched pages |
| `incomplete` | Zero or one matching candidate, but earlier or later pages remain unexamined |

An input `--cursor` makes zero or one match incomplete even on the last page: the command cannot account for earlier pages. A remaining cursor or `has_more: true` also prevents uniqueness, including when an `--all` budget is exhausted. slk combines candidates within one invocation, but does not accumulate them between commands. Resolution retains deactivated/app account flags and archived-channel status, and does not guarantee write permission.

## Open a single-person DM without sending

```sh
slk dm U0123456789 --profile work --dry-run --json
```

`dm` verifies the live identity and fetches the exact user's details. Deactivated accounts and app/bot users return `unsupported_dm_target`. The target user can belong to another team visible through Slack Connect; the command remains bound to the selected profile and does not change workspaces.

A dry-run returns `dry_run: true`, profile identity fields, and the target user object. It never calls `conversations.open`, even with `--yes`. A preview cannot establish whether a DM already exists or whether Slack will permit creation.

Without `--dry-run`, the command previews the identity and target on stderr and requests confirmation when stdin and stderr are terminals. Pressing Enter defaults to no. Non-interactive or JSON operation requires both `--profile` and `--yes`:

```sh
slk dm U0123456789 --profile work --yes --json
```

This example makes a real write. The command calls `conversations.open` with exactly one user and validates the returned DM's user, conversation ID, and type. It returns `channel_id`, `user_id`, `already_open`, and `no_op`. The last two fields are null when Slack omits them; slk does not invent a created-versus-existing result. No message is sent. Use the returned `D…` conversation ID in a separate `send` command after reviewing its target and body. Use the separate [group DM commands](conversations.md#open-resume-or-close-a-group-dm) for multiple participants.

If the server accepts the request but the response is lost or invalid, the result is `write_outcome_unknown`. Check Slack before repeating the operation. The CLI never retries it automatically.

## Permissions and compatibility

| Operation | Relevant user scopes |
| --- | --- |
| [List or inspect users](https://docs.slack.dev/reference/methods/users.info/) | `users:read` |
| [Look up an email address](https://docs.slack.dev/reference/methods/users.lookupByEmail/) | `users:read` and `users:read.email` |
| [Open or create a DM](https://docs.slack.dev/reference/methods/conversations.open/) | `im:write`, plus `users:read` for the target preview |
| Resolve a channel | The [conversation-list scopes](messages.md#authorization-and-data-use) |

These are grant prerequisites, not proof that a workspace permits an operation. Imported browser sessions remain experimental; live-workspace acceptance is pending for user, browser, and bot profiles. Bot tokens remain subject to each endpoint's token-type restrictions. No failure silently changes the API, profile, or authentication kind.
