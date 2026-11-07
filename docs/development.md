# Development

Use the toolchain declared in `rust-toolchain.toml`. The repository has a small binary entry point and an internal library. `cli` owns command dispatch, terminal input, compatibility adapters, and shared write approval; `auth` owns credential acquisition, isolated browser lifetime, persistent profiles, and injected in-memory sessions; `messages` owns validated message references, bodies, and modifications; `users` owns user records and DM results; `files` owns upload snapshots, file metadata, and bounded binary transfers; `slack` owns SDK API calls, target verification, pagination, and error mapping; `update` owns release policy and the library-backed update pipeline; `platform` owns native permissions, signals, and keyring backend selection; and `output` owns serialization and safe terminal rendering. No separate Slack Web API transport or message cache is included. Binary file transfers use reqwest without redirects or retries; the SDK still owns API requests.

Authentication arguments and terminal input live in `src/cli/auth.rs`; authentication state and acquisition remain in `src/auth`. Message rendering lives in `src/output/messages.rs`; top-level output dispatch remains in `src/output.rs`. The unpublished `xtask` crate owns release planning and native archive checks, using Cargo metadata and cargo-dist's manifest schema. The application does not depend on repository automation, and the crates.io package excludes `xtask`, workflows, orb setup, and local research files.

Credential storage uses `keyring-core` with the official `apple-native-keyring-store` on macOS, `dbus-secret-service-keyring-store` on Linux, and `windows-native-keyring-store` on Windows. The Linux backend uses encrypted Secret Service sessions and statically links libdbus. This avoids the `keyring` convenience crate's default zbus backend, which currently depends on standalone crates marked unmaintained by RustSec. No advisory is suppressed, and no plaintext backend is enabled. Explicit headless sessions bypass persistent storage rather than falling back after a keyring error.

## Checks

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build --workspace --release --locked
cargo test --workspace --locked
cargo package --locked
```

CI runs formatting, Clippy, builds, and ordinary tests on Linux, macOS, and Windows. Linux jobs also verify the Cargo package, run `cargo audit --deny warnings`, and lint workflows with pinned `jactionlint` and ShellCheck. The [release workflow](distribution.md#release-workflows) requires these checks before building or uploading archives, and requires successful crates.io publication before creating a GitHub Release. A skipped or failed prerequisite cannot approve a release.

During development, run only relevant tests; leave the full suite to CI. For release automation, use `cargo test --locked --package xtask`. The plan tests cover version/tag equality, stable versus prerelease output, license and publication gates, and target-to-runner mapping. Run `cargo run --locked --package xtask -- plan` to preview the configured matrix without publishing. `cargo package --locked --allow-dirty` permits a local uncommitted packaging check; CI and publishing never use `--allow-dirty`. Package verification compiles the unpacked crate but does not run its tests or publish it.

The known SDK acceptance failure below remains enabled. The native keyring tests are separate because they require a usable, unlocked system keyring:

```sh
cargo test --locked --lib native_keyring -- --ignored
```

The native test writes synthetic credentials under a random `slk` keyring entry, checks refresh and removal, and uses a temporary configuration directory. Use a disposable keyring when testing failure behavior. Do not unlock or modify a personal keychain to run unattended tests.

Browser acceptance tests require Chrome/Chromium and a graphical session. Set `CHROME` to the browser executable if automatic detection cannot find the installation. On a Linux build machine without a display, use Xvfb:

```sh
cargo test --locked --lib native_browser -- --ignored --test-threads=1
xvfb-run -a cargo test --locked --lib native_browser -- --ignored --test-threads=1
```

These tests use the production browser configuration, including sandboxing and HTTPS validation. Chrome's Fetch domain serves every page request from local fixture content; no Slack workspace is contacted. Tests capture synthetic JSON/form/multipart credentials, reject unrelated request origins, preserve HttpOnly cookie escapes, check remote identity mismatches with a local API server, and verify that browser data and debug listeners disappear after cleanup. They remain separate from the default unit tests because a graphical browser is an additional system prerequisite. Startup failure and CLI argument contracts run in the ordinary process tests.

Process-level tests isolate configuration paths and use synthetic credentials. SDK tests use local HTTP servers and the SDK's own connector injection; no CLI flag or environment variable can override the production API endpoint. Tests cover one-page defaults, bounded automatic pagination, unknown message/user shapes, search-path separation, name ambiguity, incomplete candidate sets, verified thread roots, exact modification targets, user/bot ownership, dry-run, explicit write approval, and authentication-expiry guidance. Terminal rendering tests preserve subsequent fields, actions, account flags, and pagination after unterminated escape sequences.

Injected-session process tests cover stdin and environment sources, all identity kinds and realms, corrupt persistent configuration that must remain untouched, missing environment values, identity mismatches, authentication-mutation rejection, and stdin conflicts. Endpoint tests check production commercial/GovSlack routing and bot Bearer transport. These tests do not establish live GovSlack availability or app-specific bot capabilities.

OAuth tests use real loopback callbacks with synthetic grants. They check S256 binding, state/path/method validation, denial/cancellation/timeout cleanup, explicit user scopes, identity mismatches, nonrotating tokens, whole-pair rotation, and uncertain grant consumption without retries. Wire-level tests require form-encoded exchange and refresh bodies, including reserved characters in authorization codes. Native keyring tests verify that the full rotation record is saved with its access token, never in config.json. These tests do not authorize a real Slack application.

File-transfer tests use an ephemeral TLS server with a certificate for both approved file hosts. Only the test client's DNS resolution and trust root change; production host validation, certificate validation, redirect policy, retry policy, and timeouts stay active. Tests exercise upload allocation, binary transfer without credentials, exact completion acknowledgements, cross-realm rejection, chunked download bounds, cancellation, timeout, rate limits, private files, and no-clobber publication. No actual file is sent to Slack.

Scheduled-message tests check absolute timezone conversion, Slack's 120-day boundary, whole seconds, thread-root resolution, previews, exact cancellation IDs, page boundaries, and uncertain acknowledgements. Conversation tests cover explicit visibility, participant sets, membership/type boundaries, unknown versus zero unread counts, cursor pages, and workspace identity. Canvas tests cover the official POST read API, section filters, one-operation edits, mandatory section IDs for replacement/deletion, byte limits, safe output, permission errors, rate limits, and read-versus-write timeout classification.

Block Kit tests exercise full-payload previews, sending, replies, editing, scheduling, explicit block removal, immutable prepared input, preservation of unknown fields, the 50-block boundary, stdin conflicts, and ownership/content rejection. Plain-text modification tests still reject implicit replacement or deletion of blocks. These tests do not establish how a real Slack client renders the supplied content or whether the app can receive interactive callbacks.

MCP uses the official `rmcp` SDK and the CLI's existing argument parser and execution path. SDK-client tests cover read-only enforcement, structured results, pagination, and rate-limit metadata with fixture-backed Slack requests. Process tests use real stdio, isolated configuration or an injected session, and no keyring credentials to check initialization, discovery, generated help, profile replacement/removal, and selector/session override rejection. Help checks include nested authentication commands, compatibility aliases, and search's inherited argument constraints; unanswered requests fail with a bounded test timeout. An MCP host and its data-retention policy still need live acceptance; initialization does not prove authorization to query Slack.

Pagination tests cross empty pages, retain ordered records and the final continuation at a budget, and reject repeated cursors or a failed later request. Splitting uses `text-splitter` rather than a custom tokenizer. Tests reconstruct Unicode/whitespace input from previews and exercise sequential sends, shared reply roots, validation before writes, and confirmed receipts after rejected or uncertain chunks. Compatibility tests verify real pinned upstream grammar, exact-message versus thread targets, chronological history, unsupported-option rejection, and unchanged approval.

Self-update uses `self_update` for release discovery, download, SHA256 verification, extraction, and replacement. Application policy requires GitHub's latest stable release, higher SemVer precedence, an exact unique target archive, and a mandatory SHA256 digest. Tests run the real tar.xz and Deflate ZIP pipeline against disposable files, including digest mismatch, HTTP failure, and missing-member controls, and verify that these pre-replacement failures preserve the old destination. They also reject plain HTTP through the production client. These are not native Windows self-replacement or real GitHub release acceptance tests; the Windows library's incomplete rollback remains documented in [distribution](distribution.md#explicit-self-update).

The disconnect test accepts each write body before closing the socket without an acknowledgement. It checks send, edit, delete, reaction and pin add/remove, DM creation, channel creation/join/leave/archive/topic/read-cursor changes, group close, Canvas edits, scheduling, and cancellation. The client must report an unknown outcome without reconnecting. Process tests check approval flags for all new mutation commands before loading credentials. Interactive default-no confirmation also needs a PTY smoke check: piped process tests deliberately reject unapproved writes before requesting input.

`.agents/setup` installs the Rust toolchain, fetches locked dependencies, and installs Linux Secret Service and Xvfb test prerequisites. Orbs already provide Chrome for Testing; use `agent-browser doctor --offline --quick` to find its executable. Setup does not unlock a keyring, import credentials, or contact Slack. Run the script from the repository root. No resume script or persistent service is required.

## Verification limits

Browser-assisted credential acquisition, PKCE OAuth and explicit rotation, manual import, reading/search, user lookup, explicit name resolution, single-person and group DMs, text and explicit Block Kit editing, text-only deletion, reactions, pins, single-file transfers, service-side scheduled messages, channel operations, explicit read cursors, limited server-provided unread information, workspace information, targeted Canvas edits, and read-only stdio MCP are implemented. The CLI provides explicit recovery guidance when credentials expire; ordinary commands never open a browser, refresh credentials, or replay a request.

Local mock tests establish request, output, and failure contracts; they do not establish Slack workspace compatibility. MVP and later-feature acceptance remain incomplete until the authorized live workflows and the known SDK acceptance test pass. New commands do not add a local workaround for the SDK defect.

Windows integration, keyring-free headless sessions, GovSlack realm binding, explicit bot identities, bounded `--all` pagination, explicit `--split` sends, verified self-update, and a documented [slackcli syntax subset](compatibility.md) are implemented. These are bounded contracts, not unrestricted platform/API equivalence. Bot search, GovSlack user RTS, bot OAuth/refresh, upstream output compatibility, implicit DM creation, and the excluded compatibility options remain unsupported. MCP supports reads only; mutations, authentication changes, file transfers, and updates remain CLI operations.

The extension checks passed on Linux: 100 relevant library tests, 15 CLI process tests, strict Clippy, a release build, and a dependency audit covering 432 dependencies. Four native browser/keyring tests stayed excluded from the ordinary test selection because they need explicit native fixtures; earlier baseline native checks do not establish acceptance of every extension. `cargo xwin check --locked --target x86_64-pc-windows-msvc --all-targets` passed, but no Windows executable, Credential Manager, DACL, console, browser, or replacement workflow has been exercised natively. macOS has not been revalidated for the complete extension set. No real Slack writes, actual self-update, release, push, or full CI run was performed for these checks.

Native Later and Slack cloud drafts lack the required public APIs. Old `stars.*` calls are not a Later implementation. Limited read-state output is not a client-equivalent unread panel. Local substitutes, Enterprise administration, whole-canvas deletion/replacement, and broader management APIs have not been selected. No local cache or persistence layer is introduced to imitate those capabilities.

`slack-morphism` 2.31.1 converts HTTP 429 responses with an empty or malformed JSON body into a JSON protocol error before returning the rate-limit status and `Retry-After`. The CLI cannot reconstruct the lost status safely. The acceptance test deliberately continues to fail with the registry dependency:

```sh
cargo test --locked --lib rate_limits_preserve_retry_after_and_make_only_one_request
```

The required result is `rate_limited` with `retry_after: 17`; the affected SDK returns `invalid_response`. On a write, the same missing response information is conservatively reported as `write_outcome_unknown`. A repair belongs in the SDK: the HTTP status and headers must determine rate limiting even when optional JSON metadata cannot be parsed. The project does not vendor an SDK fork, add a local response heuristic, mark the acceptance test ignored, or suppress its CI failure. The upstream fix [slack-morphism-rust#397](https://github.com/abdolence/slack-morphism-rust/pull/397) has merged, but the registry dependency is unchanged. Publication must wait for an SDK release containing the fix and a passing acceptance test.

The same SDK version serializes `SlackApiChatDeleteScheduledMessageRequest.scheduled_message` as `scheduled_message`, but Slack requires `scheduled_message_id`. As a temporary workaround, cancellation uses the SDK's generic POST transport with the documented JSON field instead of that request type. An upstream serde-rename fix and its serialization/deserialization regression have passed locally; the fix is not published. The generic transport retains the same identity, timeout, error, and no-retry policies. Remove this workaround when a corrected SDK release is available.

The remaining live-workspace acceptance work requires an explicitly authorized test workspace and credentials supplied through secure input, never through issues or chat:

1. Verify user, browser, and bot identities in their explicitly selected commercial or GovSlack realms. Exercise persistent and injected sessions without fallback or unintended credential storage.
2. Verify disposable-browser sign-in, organizational SSO/MFA, and the SDK's Bearer-plus-Cookie requests in the permitted workspace. Browser token/cookie acquisition is not an officially documented Slack OAuth contract; managed-browser requirements and Slack session checks may prevent replay through a CLI.
3. Confirm official RTS app eligibility, user authorization, search scopes, and applicable data-use policies.
4. Search for known messages under the supported identity kinds, and page through permitted history and threads. Check empty results, revoked access, missing scopes, expired cursors, and `--all` budgets. Confirm bot and GovSlack user search fail without fallback.
5. Preview a top-level message and a reply. Confirm that neither preview posts a message. Only with explicit write authorization, send them and verify the target, author, thread root, and exact body in Slack.
6. Query users and resolve ambiguous names across pages. Check email grants, deactivated users, and users visible from another team. Preview a single-person DM without opening it. With explicit write authorization, open an existing and a new DM, verify the returned ID, and confirm that neither operation sends a message.
7. Preview and, with explicit write authorization, modify owned text messages and react to accessible messages. Check both roots and replies, Slack's actual text/block representation, missing permissions, and deleted targets. Confirm that the thread root is not modified when the URL targets a reply.
8. Verify target desktop keyring behavior when unlocked, locked, and unavailable. Disposable native-keyring checks do not establish every desktop session's prompt behavior.
9. Authorize a PKCE-enabled app through a registered localhost callback. Verify actual user scopes, both rotating and nonrotating grants where available, explicit refresh, expiry/revocation behavior, and successful keyring replacement without a business request replay.
10. Inspect and download a permitted Slack-hosted file. With explicit write authorization, upload one private file and one shared file; verify bytes, destination, actual permissions, and completion results. Confirm external file URLs are rejected and existing local paths remain unchanged.
11. With explicit write authorization, schedule a top-level message and a reply, verify the stored UTC times, list the schedules with the same token, cancel one, and observe the other in Slack. Verify scope errors and Slack's cancellation cutoff without retrying.
12. Inspect pins, joined conversations, group membership, available read-state fields, and workspace information. With explicit write authorization, exercise selected pin, channel, group-DM, and cursor actions on disposable targets. Confirm actual membership, visibility, and client-visible cursor behavior without claiming All unreads equivalence.
13. Read an authorized Canvas, locate a section, and inspect a dry-run. With explicit write authorization, append or modify a chosen disposable section. Verify the actual section and content, access restrictions, plan limitations, and concurrent-edit behavior; do not use whole-document replacement or deletion.
14. Configure an approved MCP host for one explicit profile. Verify actual read results, errors, cancellation, and host shutdown without credential leakage or profile switching. Confirm that the host's tool-result retention complies with the data-use policy before using RTS.
15. With explicit write authorization, send a multi-chunk preview and verify exact content, ordering, top-level versus reply behavior, and recovery after partial success. Do not use automatic retries to test recovery.
16. Run native Windows and macOS checks for keyrings, protected files, cancellation, browser cleanup, and executable replacement. Test published target archives and real update failures in disposable installations before claiming release readiness; cross-compilation is not runtime acceptance.

Official search uses `assistant.search.context`, without a silent fallback to legacy `search.messages`. Browser search remains a separate experimental capability. Neither successful authentication nor mock API responses demonstrate that an app is eligible for RTS or that an organization permits browser-session automation.
