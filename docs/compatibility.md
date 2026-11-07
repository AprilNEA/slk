# slackcli syntax subset

slk accepts a bounded subset of [shaharia-lab/slackcli 0.19.0](https://github.com/shaharia-lab/slackcli/tree/3540cd6d26b8c7c74428bc6d4c1b052880900d4c) syntax. That fixed revision is the comparison baseline, not upstream's moving default branch. The adapters call the native slk workflows; they do not run slackcli, import its secrets, or reproduce its JSON schema and defaults. Native slk commands remain available.

**This is not a drop-in replacement.** Only the entries and options below are supported. Unsupported commands or options fail instead of being ignored. Identity binding, approval, pagination budgets, input validation, and Slack error semantics remain those documented for slk.

## Select an explicit identity

The `conversations`, `messages`, `search`, `users`, `files`, and `auth whoami`/`auth status` paths accept `--workspace SELECTOR`. Place the option inside that command path, not before the top-level command. The selector matches an exact saved profile name first, then a unique saved workspace/team ID. A missing or ambiguous match is an error; workspace display names, implicit default workspaces, and upstream token environment variables are not supported.

`--workspace` conflicts with `--profile`. On these paths, either explicit selector satisfies the identity requirement for automation, but an actual non-interactive or JSON write still needs `--yes`. A selector never supplies approval. Injected sessions require `--profile`, so omit compatibility `--workspace` with session injection.

Native `auth browser --workspace` and `auth oauth --workspace` still mean a validated HTTPS workspace origin, not an identity selector. The compatibility layer does not redefine those options.

```sh
slk auth whoami --workspace work --json
slk conversations list --workspace T0123456789 --limit 20 --json
slk messages send --recipient-id C0123456789 --message 'Preview only.' --workspace work --dry-run --json
```

## Supported commands

All commands accept native `--profile` and `--json`. Read paging means `--limit`, `--cursor`, `--all`, and `--max-pages`; limits and budgets use native defaults. Write options are `--dry-run` and `--yes`. A message body is exactly one of `--message TEXT` or `--message-file PATH`; `-` means stdin.

| Syntax | Supported options and behavior |
| --- | --- |
| `auth whoami` | Alias for `auth status`; optional `--workspace` |
| `auth login-auto --workspace-url ORIGIN` | Alias for isolated `auth browser`; requires native `--profile`, captures one workspace, and uses configured browser/timeouts |
| `conversations list` | Read paging; lists all native conversation types, including archived conversations |
| `conversations read CHANNEL` | Read paging; returns the fetched history oldest first |
| `conversations read CHANNEL --thread-ts TS` | Read paging; verifies and reads the thread |
| `conversations read --permalink URL` | Read paging; reads the verified thread, not only the linked message |
| `conversations get CHANNEL TS` | Reads one exact message, not channel information |
| `conversations get --permalink URL` | Reads the path timestamp; the URL's `thread_ts` is lookup context only |
| `conversations members list CHANNEL` | Read paging; no membership changes |
| `messages send --recipient-id CHANNEL` | Required body and write options; optional `--thread-ts TS` selects a verified thread |
| `messages send --permalink URL` | Required body and write options; replies to the verified root |
| `messages edit` | Exact target, required body, and write options |
| `messages delete` | Exact target and write options |
| `messages react --emoji NAME` | Exact target and write options |
| `messages unreact --emoji NAME` | Exact target and write options |
| `search messages QUERY` | Native `--limit`, `--cursor` or browser `--page`, `--all`, and `--max-pages`; optional `--workspace` |
| `users list` / `users info USER_ID` | Existing native commands with optional `--workspace`; list supports read paging |
| `files info FILE_ID` / `files download FILE_ID --output PATH` | Existing native commands with optional `--workspace`; IDs only, native output and no-clobber download rules |

An exact target is either `--permalink URL` or both `--channel-id CHANNEL` and `--timestamp TS`. The alternatives conflict. Sending requires either `--recipient-id` or `--permalink`; a permalink conflicts with both recipient and `--thread-ts`. Conversation reads likewise reject mixing the channel/thread form with a permalink.

Compatibility timestamp arguments accept native `seconds.microseconds` with six fractional digits, `p` followed by 16 digits, bare 16-digit packed timestamps, or whole seconds. The latter forms normalize to an exact string, never a float. Native timestamp flags keep their existing syntax.

## Deliberate semantic differences

- **Approval and storage:** writes do not run merely because upstream would run them. Automation requires an explicit identity and `--yes`. Credentials stay in the system keyring or an explicit in-memory session; login does not accept tokens or cookies in argv or create a plaintext secret store.
- **Targets:** channel references are explicit `C…`, `G…`, or `D…` IDs, not names, URLs, emails, or user IDs. Open a DM separately with native `dm`, then use its conversation ID. Complete message permalinks must match the selected workspace. Channel-only permalinks are rejected.
- **Threads:** reads and sends verify the root and membership before proceeding, including an explicit `--thread-ts`. Resolution failure never falls back to sending at an unverified timestamp. A bare non-broadcast reply link may need a `thread_ts` query parameter; native commands also offer `--thread ROOT_TS` for lookup context. Exact get/edit/delete/reaction targets remain the reply, never the root.
- **Bodies:** `--message-file` is a UTF-8 body, not an attachment. File and stdin whitespace is preserved, including trailing newlines. Empty or whitespace-only input is rejected. The native single-message limit applies. Compatibility send does not accept attachment `--file`, `--users`, inline `--blocks`, or `--split`; use explicit native file, Block Kit, and split workflows instead.
- **History ordering:** compatibility `conversations read` reverses the fetched history into chronological order; native `read` stays newest first. When `--all` stops at a budget, the combined history is reversed and the continuation still points to older unrequested history.
- **Pagination:** `--limit` is per page. Default page size is `settings.page_size`, not upstream's command-specific defaults. Members and users return one page unless `--all` is explicit; reaching the page budget preserves continuation metadata. This differs from upstream commands that accumulate a total number of matching records.
- **Search:** user search remains commercial RTS with cursors; browser search uses pages. Bot and GovSlack user search are unavailable. There is no legacy fallback or implicit sort change. `search messages QUERY` requires the separate query argument; native `search messages` still searches for the literal word `messages`.
- **Results:** JSON is slk JSON, and errors keep slk exit codes. Missing messages, absent reactions, and Slack permission errors remain errors rather than upstream-style successful no-ops. Reaction names use native validation, including rejection of surrounding colons. Editing and deletion retain ownership and content checks.
- **Identity selection:** only exact profile names and unique saved team IDs are supported. No workspace-name guessing, environment-driven profile selection, background update check, or default file logger is added.

## Unsupported upstream options and paths

The following upstream features are outside this subset: `--fields`; conversation type/archive filters; history time filters and self/reply exclusion; search `--in`, `--from`, `--sort`, and `--sort-dir`; user status filters and profile-field expansion; user-name/email input to `users info`; file URLs and `files read`; attachment send and implicit DM/group-DM creation; `auth list --check`; and login-auto headless, secret-backend, and timeout flags. Native equivalents, where available, retain their own syntax and behavior.

Upstream `messages schedule`, `messages list-scheduled`, and `messages delete-scheduled` are not adapted. Use native `schedule create/reply/list/cancel` with explicit conversations and absolute timestamps. Cancellation never guesses a channel from an uninspected schedule.

Names such as `conversations info`, `conversations replies`, `messages update`, `messages reactions`, and `messages pin` are not commands in the pinned upstream version and are not invented as aliases. Native `channel info`, `thread`, `edit`, `react`, and `pin` remain available. Native `users lookup` and `files upload` are slk commands, not upstream compatibility claims.

Read-only MCP accepts the supported conversation/read aliases without selectors. It rejects compatibility writes and every tool-level `--workspace`, even when the selector names the server's bound profile.
