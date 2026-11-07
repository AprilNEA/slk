# Files

File commands operate on one explicit file, not a workspace export. Every command verifies the selected profile with `auth.test`. `files info` and downloads require the applicable `files:read` grant; uploading requires `files:write` and permission to share in the selected conversation. Browser-session support remains experimental.

```sh
slk files info F0123456789 --profile work --json
slk files upload ./report.pdf --channel C0123456789 --profile work --dry-run
slk files upload ./report.pdf --channel C0123456789 --profile work --yes --json
slk files download F0123456789 --output ./downloaded-report.pdf --profile work
```

Replace the IDs and paths with explicit targets. `files info` returns available metadata but omits private download URLs. Downloads require a file ID, never a caller-supplied URL. Remote files are rejected because Slack credentials must not reach an external file provider.

## Uploads

An upload reads one regular, nonempty file into a bounded snapshot before approval. The filename must be valid UTF-8 without control characters. The approved bytes cannot change if the source file changes afterward. The preview contains the filename, byte count, selected identity, and destination conversation; it does not send bytes or request an upload URL.

Omit `--channel` to keep the file private in Slack. Sharing requires an explicit conversation ID; slk does not resolve names or create a conversation. Actual non-interactive or JSON uploads require `--profile` and `--yes`. Interactive approval defaults to no.

slk calls `files.getUploadURLExternal`, sends raw bytes to the returned HTTPS upload URL, and then calls `files.completeUploadExternal` once. The byte-transfer request carries neither the Slack Authorization header nor the session Cookie. The completion request must acknowledge the exact allocated file ID. No stage is retried. If an upload stage fails, slk does not continue to completion. An uncertain completion or an interrupted upload returns `write_outcome_unknown`; inspect Slack before repeating the operation. slk does not attempt an additional delete request to undo a partial upload.

## Downloads

Downloads accept only HTTPS URLs on the selected realm's exact file host: `files.slack.com` for commercial Slack or `files.slack-gov.com` for GovSlack, with standard port and no userinfo or fragment. Cross-realm URLs are rejected before sending credentials. Redirects are not followed. Certificate validation stays enabled. Only the validated download request receives the selected token and optional browser Cookie.

The destination must be an explicit filesystem path, not stdout. slk streams bytes to a current-user-only temporary file in the destination directory (`0600` on Unix, a protected DACL on Windows), checks the actual size against both Slack metadata and the configured limit, flushes the bytes, and publishes the file without replacement. Existing files and symlinks are never overwritten. The parent directory must already exist. Cancellation, timeout, a size mismatch, and transfer failures leave no final file and remove the temporary file. A forced process kill or OS failure can prevent cleanup.

`settings.files.max_bytes` defaults to 20 MiB for both directions. `settings.files.timeout_secs` defaults to 120 seconds per byte transfer; Slack API calls retain `settings.request_timeout_secs`. These are client limits, not promises about Slack's account or plan limits. Both values must be positive integers.

## Results and verification

Uploads return `file` metadata and `channel_id` (`null` for a private upload). Downloads return `file_id`, `path`, and the actual `size`. Private URLs never appear in JSON or text output. Text output filters terminal controls in filenames, titles, and paths; JSON preserves the original values with escaping.

Local tests use a certificate-validated TLS fixture and synthetic credentials. They cover exact bytes, upload-stage failure, missing approval, wrong identity and file IDs, redirects, rate limits, actual streaming bounds, cancellation, timeout, destination preservation, and temporary-file cleanup. Real Slack upload/download acceptance and actual scope behavior remain pending.
