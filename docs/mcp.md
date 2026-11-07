# Read-only MCP

`slk mcp --profile work` serves local Model Context Protocol tools over stdin and stdout. The MCP client starts and stops the process; slk does not install a daemon or listen on an HTTP port. Protocol negotiation, JSON-RPC, and transport use the official Rust SDK, `rmcp`.

## Configure a client

For a persistent profile, authenticate through the ordinary CLI and verify it with `slk auth status --profile work`. The MCP process must run as the same OS user, with the same configuration directory and access to the unlocked native keyring. Never put credential values in MCP configuration files, argument arrays, tool arguments, or prompts.

For clients using a `mcpServers` configuration object:

```json
{
  "mcpServers": {
    "slk-work": {
      "command": "/absolute/path/to/slk",
      "args": ["mcp", "--profile", "work"]
    }
  }
}
```

Replace the executable path and profile name. Other clients use their own configuration schema; configure the same executable and argument array as a local stdio server. Do not configure a shell command string or an HTTP URL. No token is returned by the server.

Starting with persistent storage requires an existing, explicit profile. Initialization, tool discovery, and help neither contact Slack nor read keyring credentials. Every read reloads local configuration and credentials and verifies the remote identity using the existing CLI workflow. If the profile's workspace, user, bot ID, or authentication kind changes, the running server rejects subsequent reads rather than adopting the new identity. Deleting the profile also stops subsequent reads. Configure a separate server for another identity; tool arguments cannot override `--profile`, even with the same name.

Explicitly refreshing or reimporting credentials for the same identity is reflected in subsequent reads. The server never refreshes, logs in, retries, or switches authentication kinds automatically. A keyring that requires an unavailable desktop interaction remains an error, not a reason to store credentials in plaintext.

For a headless host, have its secret manager inject the [session JSON](authentication.md#headless-sessions) into `SLK_SESSION` and configure the argument array as `["mcp", "--profile", "ci", "--session-env", "SLK_SESSION"]`. The server loads that one session into memory at startup and never accesses persistent configuration or the keyring. Restart the server to inject changed credentials. Do not include the JSON value in the client configuration; inherited environment values need protection by the launcher. `--session-stdin` is unavailable because stdin carries MCP messages.

## Tools

`slk_help` takes a `command` array of subcommand names. An empty array returns root help. The help is generated from the same Clap definitions as the CLI:

```json
{"command":["read"]}
```

`slk_read` takes an `args` array using the CLI grammar, without the executable name or `--profile`. Each array entry is one argument, including strings with spaces; slk does not invoke a shell:

```json
{"args":["read","C0123456789","--limit","10"]}
```

```json
{"args":["search","in:<#C0123456789> deploy","--cursor","NEXT_CURSOR"]}
```

The read tool supports authentication status, channel listing, search, history, exact-message reads, threads, pins, user queries and name resolution, file metadata, scheduled-message listing, channel information and members, group-DM listing, limited unread information, workspace information, and Canvas reads and section lookup. Read-only [compatibility commands](compatibility.md) are also accepted without an identity selector. The underlying scopes and API limitations do not change, including bot and GovSlack search restrictions.

Results include the CLI's JSON object in both MCP `structuredContent` and a JSON text content block. Pagination defaults to one page; an explicit `--all --max-pages N` follows the same bounded traversal as the CLI and preserves the last continuation. Command and Slack failures return `isError: true` with the CLI's error code and available error metadata. Protocol errors remain MCP errors. Adding `--json` is unnecessary. Stdout contains protocol messages only; startup failures use stderr. Cancellation stops pending asynchronous reads without replaying them.

## Boundaries

- The server rejects all writes and write previews, including `--yes` and `--dry-run`. DM open/resume, read-cursor changes, and scheduling are writes.
- File upload/download and file/stdin body inputs are unavailable. File metadata inspection does not expose private download URLs.
- Login, refresh, logout, listing other profiles, self-update (including `--check`), and nested MCP servers are unavailable. Use the CLI separately for these operations.
- Tool arguments cannot supply `--profile`, compatibility `--workspace`, `--config-dir`, or either session source, even when the value matches the bound session.
- Read-only tool annotations describe behavior; enforcement occurs before command preparation, credential access, or network requests. A client with separate shell access can still invoke the CLI, so this is not an OS sandbox or a replacement for Slack permissions.
- Returned messages, Canvas text, and Block Kit JSON are untrusted workspace data, not agent instructions. The server does not render controls, execute actions, fetch embedded URLs, or cache results.
- The MCP host and downstream agent must comply with Slack's data-use terms. In particular, do not persist or train on RTS results. A host that always retains tool responses may be unsuitable for RTS; this server cannot enforce the host's storage policy.

Local tests exercise the SDK client against actual stdio and against fixture-backed Slack requests. Compatibility with a particular agent host and authorized live workspace still requires acceptance; successful MCP initialization alone does not establish Slack permissions or RTS eligibility.
