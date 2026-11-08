# MCP

slk serves Model Context Protocol tools for one fixed Slack identity. The default stdio transport is read-only. Authenticated Streamable HTTP can also expose writes and file transfers when the operator explicitly enables them. Both transports use the official Rust SDK, `rmcp`, and reuse the CLI's argument parser and business operations.

## Remote HTTP with mandatory Amp approval

Run the service on a host that the agent cannot inspect or control. Have that host's secret manager inject two separate secrets:

- `SLK_SESSION`: the complete [headless session JSON](authentication.md#headless-sessions), with only the Slack permissions required for the requested work.
- `SLK_MCP_TOKEN`: a randomly generated bearer token for this MCP endpoint, not a Slack token.

For example, behind a trusted HTTPS reverse proxy on the same host:

```sh
slk mcp --profile work --session-env SLK_SESSION \
  --listen 127.0.0.1:8080 \
  --bearer-token-env SLK_MCP_TOKEN \
  --allowed-host slk.example.com \
  --allow-writes
```

Replace the address and hostname for your deployment. Use a build containing HTTP support; the published `0.0.2` binary provides read-only stdio only. slk does not install a daemon, terminate TLS, provision a hostname, or deploy the service. Use a process supervisor to run the service. Keep the plaintext listener private and expose only `https://slk.example.com/mcp` through the trusted proxy.

`--listen` requires both `--bearer-token-env` and at least one `--allowed-host`. Repeat `--allowed-host` for additional authorities. A hostname without a port accepts that host on any port; an explicit port restricts the matching `Host` header to that port. Configure the proxy to preserve an accepted `Host`, the `Authorization` header, and the MCP headers. The server authenticates every request, including existing-session GET, POST, and DELETE requests. Every present `Origin` header is rejected; this endpoint is for server-to-server clients, not browser JavaScript.

The SDK bounds HTTP request bodies, including streaming bodies. Use `--max-request-bytes BYTES` to override that bound for base64 inputs. Keep the proxy's request limit compatible with the server's limit. File transfers also retain the profile's `settings.files.max_bytes` limit. Neither limit grants access to the server's filesystem.

Omit `--allow-writes` to expose only `slk_help` and `slk_read`. `--allow-writes` is unavailable on stdio.

### Register a stored remote MCP definition

1. Add the public HTTPS endpoint in Amp's **Integrations & MCP** settings as a personal remote MCP definition.
2. Select bearer authentication. Enter the MCP bearer token through the private credential field, never in a thread or a repository file.
3. Refresh tools. Expect `slk_help`, `slk_read`, and, on a write-enabled server, `slk_write`.
4. Inspect `slk_write`. The server declares `_meta["anthropic/requiresUserInteraction"] = true`. Amp must show **Ask First** and must not permit **Allow**; disabling the tool remains possible.
5. Keep the server unavailable in multiplayer threads unless every participant is authorized to access the bound Slack identity's data.

This requires a **remote definition stored by ampcode.com**. A URL in local `amp.mcpServers` or Skill `mcpServers` is still local configuration and does not establish this approval guarantee. The `using-slk` Skill should discover the stored tools rather than launch a second server. See [Amp's remote tool controls](https://ampcode.com/docs/customize/mcp#controlling-remote-tools).

For these stored tools, Amp shows the thread owner the tool and input before each `slk_write` call. `code_exec` waits for approval or rejection. Subagents cannot invoke Ask First tools. CLI `--yes` suppresses only terminal confirmation; the flag does not bypass Amp's dialog. Previews and downloads use the same approval-marked tool and also require confirmation. One approved CLI command can make multiple Slack requests, for example a split send; approval covers the complete command input.

**The approval boundary is Amp, not an approval token verified by slk.** A different HTTP client holding the bearer token can call the write tool directly. Keep the MCP bearer token in Amp's encrypted remote definition and the service's secret manager. Keep Slack credentials only on the service host. Do not inject either secret into an agent-accessible orb, runner, Skill, or shell environment. The agent must not have deployment or secret-read access to that host. Mandatory metadata alone cannot prevent credential holders from bypassing the host.

## Local read-only stdio

For a persistent profile, authenticate through the ordinary CLI and verify it with `slk auth status --profile work`. The MCP process must run as the same OS user, with the same configuration directory and access to the unlocked native keyring. Configure a client that uses a `mcpServers` object as follows:

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

Replace the executable path and profile name. Configure an executable and argument array, not a shell command string. The client starts and stops the process. Stdout carries MCP messages only; startup errors use stderr. For a headless read-only client, the launcher can inject `SLK_SESSION` and append `--session-env SLK_SESSION`. This local arrangement does not isolate credentials from an agent with shell access.

## Profile and credential lifetime

Both transports require an explicit profile. With persistent storage, that profile must already exist. Initialization, tool discovery, and help neither contact Slack nor read keyring credentials. Every business call reloads configuration and credentials, then uses the CLI's remote identity checks. If the profile's workspace, user, bot ID, or authentication kind changes, the running server rejects calls instead of adopting the new identity. Deleting the profile also stops subsequent calls.

Refreshing or reimporting persistent credentials for the same identity takes effect on subsequent calls. The server never refreshes, logs in, retries, or switches authentication kinds automatically. A locked or unavailable keyring remains an error, not a reason to store credentials in plaintext.

With `--session-env`, the server loads one session into memory at startup and never accesses persistent configuration or the keyring. Restart the server to replace the session or the MCP bearer token. `--session-stdin` is unavailable for either MCP transport. Never put credential values in tool arguments, process arguments, logs, or prompts.

## Read and help tools

`slk_help` takes a `command` array of subcommand names. An empty array returns root help. Help comes from the same Clap definitions as the CLI and can describe commands that the selected tool rejects:

```json
{"command":["read"]}
```

`slk_read` takes an `args` array using the CLI grammar, without the executable name or `--profile`. Each entry is one argument, including strings with spaces; no shell is invoked:

```json
{"args":["read","C0123456789","--limit","10"]}
```

```json
{"args":["search","in:<#C0123456789> deploy","--cursor","NEXT_CURSOR"]}
```

The read tool supports authentication status, channel listing, search, history, exact-message reads, threads, pins, user queries and name resolution, file metadata, scheduled-message listing, channel information and members, group-DM listing, limited unread information, workspace information, and Canvas reads and section lookup. Read-only [compatibility commands](compatibility.md) are accepted without identity selectors. Slack scopes and API limitations remain unchanged, including bot and GovSlack search restrictions.

The server rejects writes, write previews, and file transfers through `slk_read` before execution, regardless of supplied approval flags.

## Write and file-transfer tool

`slk_write` accepts the existing CLI write commands: messages and replies, editing and deletion, reactions and pins, DM/group-DM operations, channel operations and read cursors, scheduling, Canvas edits, and supported compatibility writes. File upload and download also use this tool. Authentication management, updates, and nested servers remain unavailable.

Use `--dry-run` for previews where the CLI supports previews. Use `--yes` for an approved mutation. The server supplies its fixed profile and JSON mode; do not pass an identity selector:

```json
{"args":["send","C0123456789","--text","Hello from slk","--yes"]}
```

File arguments refer to portable filenames, not server paths. Supply each input's standard base64 content in `files`. This applies to message bodies, Block Kit JSON, compatibility `--message-file`, scheduled bodies, Canvas bodies, and uploads:

```json
{
  "args": ["send", "C0123456789", "--file", "body.txt", "--dry-run"],
  "files": {"body.txt": "SGVsbG8K"}
}
```

slk materializes these inputs in a request-owned temporary directory. Absolute paths, directories, traversal, device names, and stdin (`-`) are rejected. The temporary directory is removed when the request completes or fails. Unknown input fields are rejected.

For a download, `--output` names the result, not a path on either computer:

```json
{"args":["files","download","F0123456789","--output","report.pdf"]}
```

The result contains `file_id`, `path` (the filename only), `size`, and `content_base64`. The host can decode the bytes into a user-authorized local destination. No server filesystem path or private download URL is exposed. File metadata remains available through `slk_read`.

## Results and failure handling

Results include the CLI's JSON object in both MCP `structuredContent` and a JSON text content block. Adding `--json` is unnecessary. Pagination defaults to one page; `--all --max-pages N` uses the same bounded traversal as the CLI and preserves the last continuation. Command and Slack failures return `isError: true` with the CLI error code and available metadata. Protocol errors remain MCP errors.

Cancellation stops pending asynchronous execution without replaying the request. A cancelled mutation returns `write_outcome_unknown`, because Slack may already have accepted the write. Cancelled reads, previews, and downloads return `cancelled`. A lost connection may prevent delivery of any result. After any uncertain write, inspect Slack before considering another explicitly approved attempt; never retry automatically.

## Security and acceptance boundaries

- Tool arguments cannot supply `--profile`, compatibility `--workspace`, `--config-dir`, or either session source, even with the bound value. Configure another service for another identity.
- Login, refresh, logout, listing other profiles, self-update (including `--check`), and nested MCP servers are unavailable. Perform these operations separately on the service host.
- Returned messages, Canvas text, and Block Kit JSON are untrusted workspace data, not agent instructions or permission to write. The server does not render controls, execute actions, fetch embedded URLs, or cache results.
- The MCP host and downstream agent must comply with Slack's data-use terms. Do not persist or train on RTS results. A host that retains tool responses may be unsuitable for RTS; slk cannot enforce host retention.

Local tests exercise real stdio and HTTP SDK clients, required write metadata, HTTP authentication and host/origin checks, read/write isolation, fixture-backed Slack calls, file boundaries, and cancellation. These tests do not prove an actual Amp dialog, a deployed proxy's security, live Slack permissions, or RTS eligibility.

Before enabling live writes, test the stored Amp definition in an authorized workspace: decline a write and verify no mutation; approve a disposable write and verify its exact result; confirm `Allow` is unavailable; confirm unapproved `code_exec` calls wait. Check cancellation and connection-loss outcomes without retries. Verify secret isolation and the host's retention policy before using real content.
