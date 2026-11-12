# Authentication and output

## OAuth with PKCE

`auth oauth` authorizes a public desktop client without storing an application secret. The Slack application must already have PKCE enabled, the exact localhost redirect URL registered, and the requested user scopes configured. Enabling PKCE is an irreversible application setting; slk never changes app settings. Slack's [PKCE documentation](https://docs.slack.dev/authentication/using-pkce/) describes the prerequisites.

```sh
slk auth oauth --profile work --client-id 1234.5678 \
  --redirect-uri http://localhost:8765/oauth/callback \
  --workspace https://team.slack.com --scope chat:write,users:read
slk auth refresh --profile work
```

Use your application's public client ID and registered redirect URI. The command requests only the explicit user scopes, never bot scopes. It opens the system browser with a random state and an S256 PKCE challenge. The verifier remains in memory. Use `--no-open` to print the authorization URL on stderr instead; that option conflicts with `--json`. Run the browser and CLI on the same machine. An orb's localhost callback is not reachable from a browser on your own computer.

The temporary callback listener binds only to IPv4 loopback and accepts the registered path, GET requests, and the original state. The command rejects duplicate callback fields, validates the returned workspace/team/user with `auth.test`, and then stores the user credentials. Denial, cancellation, timeout, or validation failure does not save credentials. Ctrl+C, SIGTERM, and terminal hangup close the listener. The callback response never contains tokens, authorization codes, or state, and does not claim that persistence has succeeded.

OAuth tokens may be rotating or nonrotating according to the app's configuration. `auth refresh` requires an explicit profile and an actual refresh token from a PKCE-enabled app. It exchanges that refresh token once, verifies the same identity, and replaces the access token, refresh token, expiry, client ID, and granted scope together in one keyring value. It does not replay any business request. Ordinary commands never refresh automatically. Slack documents a 30-day lifetime for PKCE refresh tokens; if refresh expires or is revoked, authorize again.

An interrupted or uncertain exchange returns `oauth_outcome_unknown`. If Slack issued a new pair but verification or storage fails, the command returns `oauth_refresh_not_saved`. Both require fresh OAuth login rather than assuming the old refresh token remains valid. slk does not exchange an imported legacy token for a rotating token or request an application secret.

## Browser-assisted login

Browser sessions are experimental and require organizational approval. Install Chrome or Chromium and run this command from a graphical desktop session:

```sh
slk auth browser --profile work-browser --workspace https://team.slack.com
```

Complete sign-in, SSO, and MFA in the browser. The command opens a private browser context inside a new temporary profile, never a daily browser profile. slk restricts the profile directory to the current user: `0700` on Unix, or a protected current-user DACL on Windows. Chrome's sandbox and HTTPS certificate validation remain enabled. The debug endpoint binds to loopback on an OS-selected port.

Only API requests to the selected workspace can supply a candidate token. The command parses JSON, URL-encoded forms, and multipart forms with their respective parsers, then obtains the matching HttpOnly `d` cookie from Chrome. Cookie percent escapes are preserved. It does not read `localConfig_v2`, enroll additional workspaces, intercept passwords, or automate SSO/MFA. Sign-in must complete in the original login tab; popups may be used by SSO, but token capture does not follow a replacement Slack tab.

After acquisition, the command closes its browser and removes temporary data before calling `auth.test` and saving the verified credentials in the system keyring. A different workspace, bot identity, or conflicting existing profile is rejected. Missing or locked keyrings remain errors. Browser cleanup is not a Slack logout or session revocation.

Press Ctrl+C to cancel. On Unix, SIGTERM and terminal hangup also request cleanup. On Windows, console break, close, logoff, and shutdown notifications request cleanup, subject to the OS shutdown deadline. Closing the login tab cancels acquisition. Login expires after `settings.login_timeout_secs`. Startup and cleanup use `settings.request_timeout_secs`; a signal received during startup is handled after the bounded launch finishes. If graceful close fails, slk terminates only its own browser and waits for exit. Cleanup failure prevents credential storage and returns `browser_cleanup_failed`. A forced process kill, OS crash, or power loss can prevent cleanup; remove any remaining `slk-login-*` temporary directory before another login.

Chrome detection uses `settings.browser.executable` when set, otherwise the browser library's `CHROME` environment variable and standard installation paths. No browser is downloaded at runtime. `--json` suppresses progress instructions, not the browser window; success still produces one identity object and failure one error object. A managed browser that requires extensions or an existing enterprise profile may not work with a disposable profile. Use [manual import](#credential-import) explicitly when needed; there is no automatic fallback.

## Credential import

`auth login` requires an explicit `--profile` and `--kind`. Profile names contain ASCII letters, digits, periods, underscores, and hyphens. Tokens are never accepted as flags or positional arguments.

For `--kind user`, import an `xoxp-` or `xoxe.xoxp-` user token. Interactive login hides the token. With `--stdin`, provide one JSON object:

```json
{"token":"xoxp-REDACTED"}
```

For `--kind browser`, first confirm that your organization permits this use of a browser session. This path is experimental, is not an OAuth flow, and has not been validated against an authorized workspace. It uses the SDK's Bearer token and Cookie transport, not an automatic fallback to a different authentication method.

```sh
slk auth login --profile work-browser --kind browser
```

With `--stdin`, provide the workspace origin, `xoxc` token, and the value of the `d` cookie:

```json
{
  "workspace_url":"https://team.slack.com/",
  "token":"xoxc-REDACTED",
  "cookie":"xoxd-REDACTED"
}
```

Supply only the cookie value, not `d=` or a complete Cookie header. Preserve any percent escapes. Unknown fields, mismatched token kinds, and header control characters are rejected. Bot tokens require `--kind bot`; they are never accepted as user or browser credentials. Non-interactive login requires `--stdin`.

Before saving credentials, login calls `auth.test` and validates the returned workspace, team ID, user ID, realm, and authentication kind. Bot profiles must include the verified bot ID; user and browser profiles must not have one. An existing profile can refresh credentials only for the same identity and authentication kind. Use a new profile name to select a different identity.

Slack's `invalid_auth`, `not_authed`, `token_revoked`, and `token_expired` responses include explicit recovery instructions. The error still preserves `slack_error` and a failing exit code. Use `auth refresh` for rotating PKCE credentials, authorize again with `auth oauth`, obtain and import new credentials, or explicitly run `auth browser` for a browser profile. Ordinary commands never start a browser, renew credentials, switch profiles, or replay the rejected request.

## GovSlack and bot identities

Commercial Slack is the default token realm. For a GovSlack user or bot token, hidden input requires `--realm gov`; stdin JSON instead carries `"realm":"gov"`. The two selectors cannot be combined:

```sh
slk auth login --profile agency --kind user --realm gov
slk auth login --profile automation --kind bot
```

Bot import accepts `xoxb-` and `xoxe.xoxb-`. The stdin schema is the token object above, with the bot token and optional realm. Bot OAuth grants and bot refresh are not implemented; `auth oauth` and `auth refresh` remain user-only. Importing a rotating bot access token does not add refresh support.

GovSlack uses `slack-gov.com`, not `slackgov.com`. Browser login/import derives the realm from an exact `https://<workspace>.slack-gov.com` origin. OAuth derives the authorization and exchange endpoints from `--workspace`. A commercial credential cannot be used for a GovSlack identity or file, and the reverse is also rejected. Profiles and message links cannot silently change realms.

Bots can call the commands their app scopes and Slack endpoint permit. They are not personal-user substitutes: bot search and GovSlack user RTS return `unsupported_capability` without falling back to legacy search or another identity. GovSlack browser search remains experimental. History, thread access, user actions, scheduling, and Canvas permissions depend on the token kind, workspace, plan, and endpoint; successful `auth.test` does not establish these capabilities. Bot edits and deletion require the bound bot's own supported message content.

## Headless sessions

`--session-stdin` and `--session-env NAME` provide one identity-bound, process-local session without loading persistent configuration or accessing the system keyring. Both require an explicit `--profile`, conflict with each other and `--config-dir`, and never fall back to stored credentials. `NAME` is an environment variable name, not a token or JSON argument.

The injected value has this shape; the values below are placeholders, not usable credentials:

```json
{
  "kind": "user",
  "identity": {
    "workspace_url": "https://team.slack.com/",
    "team_id": "T12345678",
    "user_id": "U87654321"
  },
  "credentials": {"token": "xoxp-REDACTED"},
  "settings": {"page_size": 20, "max_pages": 5}
}
```

`kind` is `user`, `browser`, or `bot`. `credentials` uses the selected kind's import schema. A bot session also requires `identity.bot_id`. A GovSlack token session requires `credentials.realm: "gov"` and a GovSlack workspace origin. Browser credentials require the same workspace origin as the identity. Missing settings use the normal defaults. Unknown fields, incompatible identities, and invalid credentials fail before a data request.

Have a secret manager or the process launcher inject `SLK_SESSION`, then run:

```sh
slk --profile ci --session-env SLK_SESSION auth status --json
slk --profile ci --session-env SLK_SESSION channels --all --max-pages 5 --json
```

Alternatively, supply the same JSON directly to stdin with `--session-stdin`. Do not put credential values in argv, shell history, tracked files, logs, or prompts. Environment injection can be visible to the parent process and permitted OS inspection tools; use stdin when the launcher cannot protect environment values. slk keeps the session in memory but cannot control copies retained by the launcher or operating system.

Each data command verifies the declared identity through `auth.test`; injected identity metadata does not grant authority. Write approval is unchanged: non-interactive or JSON writes need `--profile` and `--yes`. `auth list` shows only the injected session. Login, logout, browser acquisition, OAuth, and refresh reject in-memory sessions instead of persisting or changing credentials. Supply a fresh session explicitly when credentials change.

Session stdin cannot also supply a message body, blocks, compatibility `--message-file -`, login JSON, or MCP transport. Use environment injection for [MCP](mcp.md), including HTTP mode, or when the body must come from stdin. MCP captures the injected session at startup; restart the server to replace it. For write-enabled remote MCP, keep Slack credentials on the isolated service host and the separate MCP bearer token in the client's protected remote definition, never in an agent-accessible environment.

## Profiles and storage

- `auth list` reads local metadata without contacting Slack or reading secrets. Listed profiles are not necessarily still authenticated.
- `auth status` verifies the selected identity with Slack. It can omit `--profile` only when exactly one profile exists. It does not test search scopes, app eligibility, or message permissions.
- `auth logout` requires `--profile` and removes local credentials and metadata. It does not revoke a Slack token or browser session. Revoke compromised credentials in Slack as well.

Persistent tokens and cookies, or OAuth access and refresh tokens, are stored together in the system keyring under the `slk` service. Linux uses Secret Service, macOS uses Keychain, and Windows uses Credential Manager. Missing, locked, or unavailable keyrings produce errors, never an automatic headless or plaintext fallback. New entries are removed if saving profile metadata fails; a failed cleanup produces a separate error rather than a successful login.

Metadata lives in `config.json` under the platform configuration directory:

- Linux: `$XDG_CONFIG_HOME/slk`, or `~/.config/slk`.
- macOS: `~/Library/Application Support/slk`.
- Windows: `%APPDATA%\slk\config`.

Use `--config-dir PATH` to select another persistent configuration directory. The file contains `settings` and a `profiles` object. Each profile contains its `kind`, `identity` (workspace URL, team ID, user ID, and optional bot ID), and a non-secret `keyring_ref`. Do not add credentials to this file. Invalid configuration is reported rather than replaced. On Unix, the application tightens directory and file permissions to `0700` and `0600`, including existing paths. On Windows, it applies a protected DACL granting access to the current user. Symlink configuration paths are rejected.

Timeouts and page size accept positive integers; the optional browser executable is a filesystem path:

| Setting | Default | Meaning |
| --- | --- | --- |
| `settings.request_timeout_secs` | 30 | Timeout for each Slack API request, browser startup/control request, and graceful browser shutdown |
| `settings.page_size` | 20 | Default page size; capped at 20 for search and 999 for conversation and user-list APIs |
| `settings.max_pages` | 100 | Maximum page requests after an explicit `--all`; ordinary reads still request one page |
| `settings.login_timeout_secs` | 300 | Browser-acquisition budget including startup, or time waiting for the OAuth callback; excludes cleanup and API verification |
| `settings.browser.executable` | `null` | Explicit Chrome/Chromium executable; `null` enables automatic detection |
| `settings.files.max_bytes` | 20971520 | Maximum bytes in one file upload or download (20 MiB) |
| `settings.files.timeout_secs` | 120 | Timeout for each binary file transfer, separate from Slack API requests |
| `settings.messages.chunk_chars` | 4000 | Maximum Unicode scalar values per chunk for explicit `--split`; cannot exceed the single-message limit |
| `settings.messages.max_chunks` | 10 | Maximum chunks allowed before a split send is rejected without writes |

An explicit `--limit` overrides the page-size default. Slack can return fewer results or apply stricter limits. Preserve the `profiles` object when editing settings. Configuration writes use an atomic file replacement. Authentication mutations hold an exclusive `auth.lock` file lock and reload metadata after acquisition; a concurrent login, refresh, or logout fails with `authentication_busy`. The operating system releases the lock when the process exits. Reads and `auth list` do not acquire the lock or trigger network refreshes.

## Network boundary

User and bot tokens target `https://slack.com/api` or `https://slack-gov.com/api` according to their explicit realm. Browser credentials only target `/api` at their validated workspace origin in that realm. Credentials cannot select an arbitrary API URL. HTTP origins, userinfo, nonstandard ports, extra host labels, non-Slack domains, paths, query strings, and fragments are rejected.

File download credentials additionally target the exact HTTPS host `files.slack.com` or `files.slack-gov.com` in the same realm, only after validating a URL obtained through `files.info`. Upload byte transfers carry no token or Cookie. File transfers do not follow redirects or retry; external files are rejected. See [file commands](files.md) for size limits and destination handling.

Slack SDK requests run sequentially and do not follow redirects. SDK rate-control retries are disabled, and the CLI does not retry requests. The interactive login browser follows redirects and loads Slack/SSO resources as a browser normally does. A timeout or HTTP failure is an error, never a successful empty result. Every data command verifies the live identity against the selected profile before querying data or writing. Message URLs are parsed locally and are never fetched with credentials.

The explicit `update` command uses an independent HTTPS client for this project's GitHub releases. It follows bounded HTTPS redirects for release assets, sends no Slack credentials, and does not run during ordinary commands. See [update verification and replacement limits](distribution.md#explicit-self-update).

An HTTP 429 response to a Slack API request returns `rate_limited`, with `retry_after` when Slack sends `Retry-After`, even when the response body is empty or not valid JSON. For an uncertain write result, the CLI returns `write_outcome_unknown`; check Slack before repeating any send, DM open, edit, deletion, reaction, or upload.

## Text and JSON

The default is text even when stdout is piped. `--json` explicitly selects JSON. Successful commands write one result to stdout. Failed commands leave stdout empty and write one error object to stderr. Help and version requests remain text and exit successfully.

Login and status return identity fields:

```json
{"profile":"work","kind":"user","workspace_url":"https://team.slack.com/","team_id":"T12345678","user_id":"U87654321"}
```

Bot identities also include `bot_id`. `auth list` returns `{"data":[...]}` with the same profile fields. Logout returns the profile and `local_credentials_removed: true`. Authentication results never serialize stored tokens, cookies, or keyring contents. [Message commands](messages.md#json-results) and [user commands](users.md) return their own result structures.

Errors contain a stable `code` and `message`, plus `slack_error` or `retry_after` (seconds) when available. For example:

```json
{"code":"slack_error","message":"Slack rejected the request.","slack_error":"missing_scope"}
```

Exit codes are `0` for success, `1` for runtime failures, and `2` for invalid arguments or supplied credentials. Raw SDK response bodies, keyring diagnostics, and argument values are not echoed in error messages. No debug logger is installed. Text rendering strips terminal escape sequences and controls other than newlines and tabs from each field before combining fields. An unterminated sequence cannot hide later fields, messages, or pagination notices. JSON preserves the original field values with standard escaping; consumers that replay raw JSON string values must still handle untrusted text safely.
