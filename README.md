# slk

A personal Slack CLI for developers, scripts, and coding agents, written in Rust.

Read and search Slack, work with messages, users, files, channels, schedules, and Canvas documents, or expose read-only tools to a local MCP client. Writes require approval. Persistent credentials stay in the system keyring; headless sessions use explicit in-memory injection. slk does not cache messages or install a background service.

**Experimental.** Releases retain a known HTTP 429 parsing defect in `slack-morphism`.

The affected responses lose their rate-limit classification and `Retry-After`; the regression test is temporarily ignored while awaiting a corrected SDK release. Passing CI does not establish that this defect is fixed or that live Slack workflows work. Browser login and browser search are experimental. See [verification limits](docs/development.md#verification-limits) and [release setup](docs/distribution.md#release-workflows).

## Install

Use current stable Rust on macOS, Linux, or Windows with the MSVC C++ build tools. A C compiler and linker are required; Linux builds compile the bundled libdbus:

```sh
cargo install slk --locked
slk --help
```

To build a local checkout instead, use `cargo install --path . --locked`.

Persistent authentication requires an unlocked macOS Keychain, Linux Secret Service, or Windows Credential Manager. Browser login additionally requires Chrome/Chromium and a graphical session. There is no plaintext credential fallback. See [distribution](docs/distribution.md) for local archives, platform requirements, and explicit self-update.

## Use

```sh
slk auth login --profile work --kind user
slk auth status --profile work --json
slk channels --profile work --limit 20 --json
slk read C0123456789 --profile work --limit 20
slk send C0123456789 --profile work --text 'Hello from slk' --dry-run
```

Replace the conversation ID with one from your workspace. Login reads credentials through hidden prompts or stdin JSON, never token arguments. [Authentication](docs/authentication.md) also covers PKCE OAuth, isolated browser login, bot identities, GovSlack, and headless sessions. User search uses Slack's Real-time Search API (RTS), which requires an eligible app and explicit user scopes.

`--dry-run` makes no write request. Actual non-interactive or JSON writes require both `--profile` and `--yes`. Reads return one page by default; `--all --max-pages 5` opts into bounded pagination. Requests never retry automatically. When a write has an unknown outcome, check Slack before repeating it.

Run `slk mcp --profile work` for read-only stdio tools bound to one identity. MCP does not expose writes, file transfers, authentication changes, or an HTTP endpoint. See [MCP setup and data-use boundaries](docs/mcp.md).

Command references: [messages](docs/messages.md), [users](docs/users.md), [conversations](docs/conversations.md), [files](docs/files.md), [Canvas](docs/canvas.md), and the [slackcli compatibility subset](docs/compatibility.md). See [development](docs/development.md) for module ownership, checks, and acceptance limits.

## License

Licensed under either the [MIT License](LICENSE-MIT) or the [Apache License, Version 2.0](LICENSE-APACHE), at your option.
