# Message commands

Every command selects a persistent or explicitly injected profile and verifies its complete identity with `auth.test`. A single stored profile is selected automatically for reads and previews; multiple profiles and injected sessions require `--profile`. Message commands do not join channels, open DMs, change identity, or retry an API request. Opening a DM is a separate, explicit [user command](users.md#open-a-single-person-dm-without-sending).

## Read messages

```sh
slk channels --profile work --limit 20 --json
slk search 'in:<#C0123456789> deploy' --profile work --limit 10 --json
slk read C0123456789 --profile work --cursor 'CURSOR_FROM_PREVIOUS_PAGE' --json
slk thread 'https://team.slack.com/archives/C0123456789/p1700000000000001' --profile work --json
slk get 'https://team.slack.com/archives/C0123456789/p1700000000000002?thread_ts=1700000000.000001' --profile work --json
```

`channels` requests public channels, private channels, group DMs, and existing DMs visible to the selected identity. It returns conversation IDs and types, including archived conversations. Use the ID, not a display name or user ID, for `read` and `send`.

`search` passes the query to Slack without interpreting it locally. With a commercial `user` profile, it calls `assistant.search.context` for messages, including bot messages. It does not fetch context messages automatically. With a `browser` profile, it calls experimental legacy `search.messages`. Bot search and GovSlack user RTS are unavailable. A missing scope, expired session, or unavailable capability is an error, never a fallback to another search API.

`read` returns history in Slack's newest-first order. `thread` returns the verified thread in oldest-first order. The first thread page normally contains the root; subsequent pages can omit it. Returned `blocks` remain opaque JSON, including unknown block types and fields; text output escapes the JSON rather than rendering interactive controls. Unknown message subtypes and other extra fields are ignored. Missing text is represented by an empty string. RTS search requests text without blocks; use `read` or `thread` to inspect a message's rich content.

`get` returns one exact message object, not a page or the entire thread. In the example above, it reads `1700000000.000002`; `thread_ts` only supplies lookup context. For a bare reply link, provide `--thread ROOT_TS` when needed. Missing messages and unverifiable thread membership remain errors. The [slackcli compatibility subset](compatibility.md) provides `conversations get` and a chronological history adapter without changing native `read` ordering.

### Pagination

| Command or profile | Continuation input | JSON pagination | Explicit `--limit` |
| --- | --- | --- | --- |
| `channels`, `read`, `thread` | `--cursor` | `next_cursor`, and `has_more` when Slack supplies it | 1–999 |
| `search` with `user` | `--cursor` | `next_cursor` | 1–20 |
| `search` with `browser` | `--page` (1–100) | `next_page`, `has_more` | 1–20 |

The default page size is `settings.page_size`, capped at the method's limit. Slack can impose a smaller limit. A short or empty page can still have a continuation: inspect pagination instead of counting results. A null `next_cursor` means no cursor was supplied. If `has_more` is true without a continuation, the result must not be treated as complete. Browser search stops at page 100; when more pages exist, it returns `next_page: null` and `has_more: true`. Narrow the query to reach those results.

Each invocation retrieves one result page by default, in addition to identity and thread-target checks. Add `--all` to follow continuations sequentially, bounded by `settings.max_pages` (default 100), or set a per-invocation budget:

```sh
slk read C0123456789 --profile work --all --max-pages 5 --limit 20 --json
slk search 'in:<#C0123456789> deploy' --profile work --all --max-pages 3 --json
```

`--max-pages` requires `--all`. `--limit` remains the per-page limit, not the total number of results. Traversal starts from the supplied cursor or page when present. Empty pages with a continuation do not end traversal. The combined result preserves API order and the last page's continuation metadata, including when the budget is exhausted. A missing continuation ends traversal even if `has_more` remains true; that result is still incomplete. A repeated cursor or failed request fails the whole read without returning a successful partial result or retrying.

The same traversal options apply to users, name resolution, conversation members, group-DM lists, unread lists, and scheduled-message inspection. Results can change between requests; pagination is not a snapshot or export mechanism. Larger budgets increase API calls and memory use and remain subject to rate limits and Slack data-use terms.

### Thread targets

`thread` and `reply` accept HTTPS Slack message URLs such as:

```text
https://team.slack.com/archives/C0123456789/p1700000000000002?thread_ts=1700000000.000001
```

The URL's workspace must match the selected profile. The CLI parses the URL locally and never follows it. Timestamps remain strings with six fractional digits; no floating-point conversion occurs.

The CLI verifies the root with exact-timestamp history requests. For a reply URL with a `thread_ts` hint, it also verifies that the referenced reply belongs to that thread. A broadcast reply visible in history can identify its parent, which is then independently checked. A bare, non-broadcast reply URL may be absent from history; the CLI returns `thread_unresolved` instead of treating that reply as a root. Supply the actual root timestamp:

```sh
slk thread 'https://team.slack.com/archives/C0123456789/p1700000000000002' --profile work --thread 1700000000.000001
```

An explicit `--thread` must agree with any `thread_ts` in the URL. A nonexistent root or unrelated reply is rejected. Known non-threadable `channel_join` and `channel_leave` events also return `thread_unresolved`, including during `reply --dry-run`. These events remain visible in ordinary history. Unknown subtypes still undergo the normal timestamp and membership checks. Deleted or inaccessible messages can prevent resolution even when a timestamp is known.

## Preview and send

```sh
slk send C0123456789 --profile work --text 'Deployment completed.' --dry-run
slk reply 'https://team.slack.com/archives/C0123456789/p1700000000000001' --profile work --file reply.txt --dry-run --json
```

`--dry-run` verifies identity and resolves thread targets, then returns the profile, workspace, user, conversation, thread root, and exact body. It never calls `chat.postMessage`. A top-level send preview does not prove that the conversation is accessible or writable; Slack checks that on the actual post.

Body inputs are mutually exclusive: `--text STRING`, `--file PATH`, or `--file -` for stdin. Files and stdin must be UTF-8. Empty or whitespace-only bodies are rejected. The body is never passed to a shell. Quote `--text` in your own shell, or use a file to avoid shell expansion before slk starts.

slk accepts at most 4,000 Unicode scalar values per message. This is a conservative application limit, not Slack's 40,000-character truncation threshold: [Slack documents that longer text may be split into multiple messages](https://docs.slack.dev/reference/methods/chat.postMessage/#truncating). Without explicit `--split`, slk rejects longer bodies rather than truncating, splitting, or uploading them as files.

With stdin and stderr attached to a terminal, an actual write previews the target and body on stderr and asks for confirmation. Pressing Enter defaults to no; cancellation sends nothing. `--yes` explicitly skips confirmation. Non-interactive or JSON writes require both `--yes` and an explicit `--profile`, even when only one profile exists:

```sh
slk reply 'https://team.slack.com/archives/C0123456789/p1700000000000001' --profile work --file reply.txt --yes --json
printf '%s\n' 'Deployment completed.' | slk send C0123456789 --profile work --file - --yes --json
```

These two examples perform real writes. `send` creates a top-level message; `reply` uses the verified root and never broadcasts the reply. Requests contain text and optionally explicit Block Kit JSON, not attachments. Link and media unfurls and automatic mention linking are disabled. Slack's text formatting remains enabled; explicit mention syntax in the body can still notify people. Inspect the body before approval. Confirmation is an accident-prevention measure, not a permission boundary against an agent that can supply `--yes`.

Success returns the acknowledged conversation and timestamps without a second permalink request. Connection failures, timeouts, HTTP 5xx responses, ambiguous Slack internal errors, and invalid success responses return `write_outcome_unknown`. Slack may already have accepted the message: check the conversation before sending again. No error triggers an automatic retry, and manually rerunning a write can duplicate it.

### Explicit long-message splitting

Native `send` and `reply` accept `--split` for plain text:

```sh
slk send C0123456789 --profile work --file report.txt --split --max-chunks 5 --dry-run --json
slk reply 'https://team.slack.com/archives/C0123456789/p1700000000000001' --profile work --file report.txt --split --chunk-chars 2000 --dry-run
```

The splitter prefers text boundaries and counts Unicode scalar values. `--chunk-chars` defaults to `settings.messages.chunk_chars` (4,000); `--max-chunks` defaults to `settings.messages.max_chunks` (10). Both overrides require `--split`. The complete body and every chunk are validated before any write. Exceeding either limit or producing a whitespace-only chunk rejects the operation. Whitespace and order are preserved; no chunk numbering or Markdown repair is inserted. Code fences and Slack formatting can span chunk boundaries, so inspect the preview.

Approve the whole ordered preview once. A split `send` creates separate top-level messages; a split `reply` sends every chunk to the same verified thread root. The first chunk does not become a new thread root. Blocks, editing, scheduling, and compatibility `messages send` do not support splitting.

Writes run sequentially and stop on the first failure or uncertain acknowledgement. Already accepted chunks are not deleted or resent. A multi-chunk failure returns `split_write_failed` with `batch.sent` (confirmed receipts), one-based `batch.failed_chunk`, `batch.total_chunks`, and `batch.failure` containing the original error. The failed chunk may also have been accepted when the nested code is `write_outcome_unknown`. Check Slack before resuming; rerunning the full input can duplicate earlier chunks.

### Block Kit messages

`send`, `reply`, `edit`, `schedule create`, and `schedule reply` accept `--blocks-file PATH`. The file contains the complete JSON array, not a request object with a `blocks` property:

```json
[
  {"type":"section","text":{"type":"mrkdwn","text":"*Deployment completed.*"}}
]
```

```sh
slk send C0123456789 --profile work --text 'Deployment completed.' --blocks-file blocks.json --dry-run
slk edit 'https://team.slack.com/archives/C0123456789/p1700000000000002?thread_ts=1700000000.000001' --profile work --text 'Deployment completed.' --blocks-file blocks.json --dry-run
```

The ordinary `--text` or `--file` body remains required as the accessible notification fallback. Include all information screen-reader users need in that text. `--blocks-file -` reads JSON from stdin; text and blocks cannot both consume stdin. slk reads the inputs once before approval and sends the same values, even if the file changes afterward.

The array accepts up to Slack's 50 message blocks. Every entry must be an object with a nonempty string `type`. slk preserves all supplied fields instead of round-tripping through a narrower block model. Slack validates nested schemas, per-block size limits, supported surfaces, interactivity configuration, and permissions. Invalid blocks remain errors; slk never retries as plain text. Previews show fallback text and the complete escaped JSON, not a Slack-rendered view. They do not fetch images or execute button actions. Interactive elements require the appropriate Slack app configuration; slk does not host an interaction callback server.

For editing, the supplied array replaces all existing blocks. An explicit `[]` removes the blocks and uses the new text; omitting `--blocks-file` still rejects a target with nonempty blocks. Preview the original blocks and the complete replacement before approval. Slack can reject a rich-text/non-rich-text conversion with `block_mismatch`; slk does not change the payload to bypass that restriction. A preview is not concurrency protection against another editor.

## Edit, delete, or react to the exact message

```sh
slk edit 'https://team.slack.com/archives/C0123456789/p1700000000000002?thread_ts=1700000000.000001' --profile work --text 'Corrected text.' --dry-run
slk delete 'https://team.slack.com/archives/C0123456789/p1700000000000002?thread_ts=1700000000.000001' --profile work --dry-run
slk react 'https://team.slack.com/archives/C0123456789/p1700000000000002?thread_ts=1700000000.000001' thumbsup::skin-tone-3 --profile work --dry-run
slk unreact 'https://team.slack.com/archives/C0123456789/p1700000000000002?thread_ts=1700000000.000001' thumbsup::skin-tone-3 --profile work --dry-run
```

All four commands inspect and operate on the URL's exact message timestamp, not its thread root. In these examples the target is `1700000000.000002`, not `1700000000.000001`. For a reply URL without `thread_ts`, supply its parent with `--thread`. The parent is lookup context only; it never changes the modification target. Missing, deleted, inaccessible, and unrelated messages produce an error rather than a guessed target.

`edit` and `delete` require the selected identity to be the message author. User profiles reject bot-authored content; bot profiles can modify their own messages when the bot identity matches, including the `bot_message` subtype. Both reject attachments, files, and other event subtypes except `thread_broadcast`. Plain-text editing and deletion reject nonempty blocks even when the message also has `text`. Editing blocks requires an explicit `--blocks-file` and previews the original and replacement JSON; deleting a Block Kit message is not supported. Ownership rejection returns `message_not_owned`; unsupported content returns `unsupported_message`.

Editing uses the same body inputs and single-message limit as sending. Its preview includes both the original message and replacement text. The request disables automatic mention linking and parsing but preserves Slack text formatting; explicit mentions can still notify people. Deletion previews the original text and removes only the specified message. There is no bulk deletion.

`react` adds a reaction; `unreact` removes the selected identity's reaction. Other users' and bot-authored messages can be reaction targets when accessible. Pass an emoji name without surrounding colons, whitespace, or controls. Skin-tone names such as `thumbsup::skin-tone-3` remain intact. Slack validates the emoji and permissions. Errors such as `already_reacted`, `no_reaction`, and `not_reactable` remain errors, not success results.

All four commands share the send command's confirmation rules. Dry-runs inspect identity and target but never call a mutation endpoint. An interactive write previews the operation and defaults to no; non-interactive or JSON writes require both `--profile` and `--yes`. The commands never retry, including after a timeout or lost acknowledgement. `write_outcome_unknown` means Slack may already have applied the change; inspect Slack before repeating it. A preview cannot prevent another client from changing or deleting the target before the write.

## Pin messages for a conversation

```sh
slk pins C0123456789 --profile work --json
slk pin 'https://team.slack.com/archives/C0123456789/p1700000000000002?thread_ts=1700000000.000001' --profile work --dry-run
slk unpin 'https://team.slack.com/archives/C0123456789/p1700000000000002?thread_ts=1700000000.000001' --profile work --dry-run
```

`pins` returns `channel_id` and `data`, preserving all item kinds returned by `pins.list`, including legacy file items and unknown future shapes. It does not interpret opaque items as messages or silently drop them. The API response has no cursor pagination. Text output renders each item as escaped JSON.

`pin` and `unpin` inspect the exact linked message and use the same identity, thread-context, preview, approval, and no-retry rules as reactions. They send Slack's `timestamp` parameter for that message, never the thread root. Both can target another author's accessible message; Slack checks pin permissions and rejects unsupported targets. The write commands pin messages only, not files or file comments. Already-pinned, not-pinned, and permission errors remain errors.

Pins are visible to the conversation's members. They are not private bookmarks or Slack Later. Unpinning removes the pin, not the message. The CLI does not expose obsolete `stars.*` calls as an implementation of Later.

## Schedule on Slack

```sh
slk schedule create C0123456789 --at '2026-11-07T09:00:00+08:00' --profile work --text 'Release reminder.' --dry-run
slk schedule reply 'https://team.slack.com/archives/C0123456789/p1700000000000001' --at '2026-11-07T01:00:00Z' --profile work --file reply.txt --dry-run
slk schedule list C0123456789 --profile work --limit 20 --json
slk schedule cancel C0123456789 Q0123456789 --profile work --dry-run
```

Replace the example dates with a future time. `--at` requires an RFC3339 timestamp with an explicit timezone and whole seconds, no more than 120 days in the future. The preview includes both the absolute UTC time and Unix timestamp. The CLI checks the time again after approval. Slack enforces additional scheduling limits, including its per-channel scheduling window; a server rejection is not retried.

`create` and `reply` share immediate messages' body inputs, target verification, disabled unfurls, non-broadcast replies, and approval rules. Only an approved request calls `chat.scheduleMessage`. Slack owns the schedule; slk does not stay running or install a timer. Success returns `channel_id`, `scheduled_message_id` as a string, and integer `post_at`.

`list` requests one page in one explicit conversation by default; `--all` opts into bounded traversal. Slack only returns schedules created through the API with that token, not schedules created in Slack's UI or with another token. `cancel` first finds the exact ID in the requested pages and previews its stored text and time; this API does not provide the scheduled Block Kit payload, so the preview is not a full rich-content inspection. If the schedule is on another page, pass that page's `--cursor` or explicitly use `--all --max-pages N`. A missing or duplicate match cannot be cancelled, and pagination never cancels additional schedules. Slack rejects cancellation within 60 seconds of posting. A schedule may fire or change between inspection and cancellation.

These commands use `chat:write`. Reads and previews do not create or cancel schedules. An uncertain create or cancellation returns `write_outcome_unknown` without retrying; inspect Slack before repeating the request. See [the SDK serialization limitation](development.md#verification-limits) for cancellation's current generic-POST adapter.

## JSON results

Paginated read commands return `data` and `pagination`; `get` returns a single message object. Message results preserve IDs and timestamps as strings:

```json
{
  "data": [{
    "channel_id": "C0123456789",
    "ts": "1700000000.000002",
    "thread_ts": "1700000000.000001",
    "author": "U0123456789",
    "text": "Deployment completed.",
    "permalink": null
  }],
  "pagination": {"next_cursor": "NEXT_CURSOR", "has_more": true}
}
```

`thread_ts`, `author`, and `permalink` can be null. `author` is a user ID or bot ID when available. `blocks` is included only when Slack returns it, preserving the JSON unchanged. History and thread responses need not include permalinks; slk does not request one separately per message. `channels` uses the same page envelope with `channel_id`, `name`, `conversation_type`, `user_id`, and `is_archived` fields.

Successful sends return:

```json
{"channel_id":"C0123456789","ts":"1700000000.000009","thread_ts":"1700000000.000001"}
```

`thread_ts` is null for top-level sends. Send/reply dry-run results contain `dry_run: true`, profile identity fields, `channel_id`, `thread_ts`, `text`, and `blocks` when supplied.

For multiple chunks, a dry-run instead returns `dry_run`, profile identity fields, and a `drafts` array containing every complete chunk and target. Success returns `data` with the ordered receipts. A split input that fits in one chunk retains the single-message result shape.

Modification dry-runs contain `dry_run: true`, profile identity fields, the original `message` object, and `action` (`edit`, `delete`, `react`, `unreact`, `pin`, or `unpin`). Editing also includes replacement `text` and optional `blocks`; reactions include `name`. Success returns the exact `channel_id` and `ts` with the same action fields. The returned edit payload is the requested content, not a follow-up read of Slack's normalized message:

```json
{"channel_id":"C0123456789","ts":"1700000000.000002","action":"edit","text":"Corrected text."}
```

JSON failures leave stdout empty and write one error object to stderr; see [output and exit codes](authentication.md#text-and-json).

## Authorization and data use

Authorize an eligible PKCE app with `slk auth oauth`, or import a user token obtained through your app's authorized Slack flow. slk does not create or configure the app. RTS is available to internal or directory-published apps, requires user authorization when called outside Slack, and is not enabled by the `xoxp` prefix alone. Review the current [RTS requirements and data policy](https://docs.slack.dev/apis/web-api/real-time-search-api/).

| Operation | Relevant user scopes |
| --- | --- |
| [List all conversation types](https://docs.slack.dev/reference/methods/conversations.list/) | `channels:read`, `groups:read`, `im:read`, `mpim:read` |
| [Search messages with RTS](https://docs.slack.dev/reference/methods/assistant.search.context/) | `search:read.public`, `search:read.private`, `search:read.im`, `search:read.mpim` |
| [History and thread access](https://docs.slack.dev/reference/methods/conversations.replies/) | The conversation's corresponding `channels:history`, `groups:history`, `im:history`, or `mpim:history` |
| [Send a message](https://docs.slack.dev/reference/methods/chat.postMessage/) | `chat:write`, plus the conversation permissions |
| [Edit](https://docs.slack.dev/reference/methods/chat.update/) or [delete](https://docs.slack.dev/reference/methods/chat.delete/) | `chat:write`, plus history access for target inspection |
| [Add](https://docs.slack.dev/reference/methods/reactions.add/) or [remove](https://docs.slack.dev/reference/methods/reactions.remove/) a reaction | `reactions:write`, plus history access for target inspection |
| [List pins](https://docs.slack.dev/reference/methods/pins.list/) | `pins:read` |
| [Pin](https://docs.slack.dev/reference/methods/pins.add/) or [unpin](https://docs.slack.dev/reference/methods/pins.remove/) a message | `pins:write`, plus history access for target inspection |
| [Schedule](https://docs.slack.dev/reference/methods/chat.scheduleMessage/), [list](https://docs.slack.dev/reference/methods/chat.scheduledMessages.list/), or [cancel](https://docs.slack.dev/reference/methods/chat.deleteScheduledMessage/) scheduled messages | `chat:write`, plus history access for reply target inspection |

The CLI requests all four conversation types for listing and search. Missing grants can reject the request; it does not silently narrow the search. Replies, modifications, and their previews also need history access to verify the target. Slack can apply additional app, workspace, membership, and data-access restrictions.

Browser profiles use an explicitly imported or isolated-browser session only when your organization permits that use. The Bearer-plus-Cookie transport and legacy search path remain unverified against an authorized workspace; they are not a promise of support or a policy bypass. Ordinary commands never open a browser or refresh a session implicitly.

slk does not cache search results or provide bulk export. Slack's RTS policy prohibits storing or copying retrieved data or using it for training. Downstream scripts, agent context stores, persistent logs, and other services must independently comply with applicable Slack terms and organizational policy.
