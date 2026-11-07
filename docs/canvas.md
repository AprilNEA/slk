# Canvas commands

Canvas commands use Slack's public `canvases.getContent`, `canvases.sections.lookup`, and `canvases.edit` APIs through the SDK transport. They verify the selected user and workspace before accessing a canvas. They do not render HTML, start a browser, execute document content, or change access permissions.

## Read content and find sections

```sh
slk canvas read F0123456789 --profile work --json
slk canvas read F0123456789 --format html --profile work --json
slk canvas sections F0123456789 --contains 'Release plan' --section-type h1,h2 --profile work --json
```

Use an explicit canvas ID beginning with `F`, not a URL. Reading returns the complete document in one response without pagination. The default format is Markdown; HTML is returned as data, not rendered. JSON contains `canvas_id`, `content_type`, and `content`. An empty canvas is an empty string, not an error. Text output filters terminal controls independently from each other field; JSON preserves the source content.

`sections` requires `--contains`, `--section-type`, or both. Section types are comma-separated Slack filter names, such as `h1,h2` or `any_header`; Slack validates their availability. The response contains `canvas_id` and `sections`, each with an opaque `id`. This is a search for matching section IDs, not a guaranteed enumeration of every section or the section bodies. Copy the intended ID for an explicit edit.

## Preview one edit

```sh
slk canvas append F0123456789 --file addition.md --profile work --dry-run
slk canvas prepend F0123456789 --text '# Overview' --profile work --dry-run
slk canvas insert-after F0123456789 'temp:C:SECTION_ID' --file addition.md --profile work --dry-run
slk canvas insert-before F0123456789 'temp:C:SECTION_ID' --file addition.md --profile work --dry-run
slk canvas replace-section F0123456789 'temp:C:SECTION_ID' --file replacement.md --profile work --dry-run
slk canvas delete-section F0123456789 'temp:C:SECTION_ID' --profile work --dry-run
```

Replace the example section ID with an actual `sections` result. Each command represents exactly one Slack edit operation. `replace-section` and `delete-section` require a nonempty section ID; omitting it is an argument error, never permission to replace or delete the entire document. Whole-canvas replacement, deletion, creation, renaming, and access management are not exposed.

Except for deletion, edits accept exactly one of `--text`, `--file PATH`, or `--file -`. slk requires nonempty Markdown and applies a conservative limit of 1 MiB of UTF-8 bytes per edit. It neither truncates nor splits content. Slack's supported Canvas Markdown differs from Block Kit and ordinary message formatting; unsupported markup remains a Slack error.

Every edit reads the current full Markdown document before approval, including when `--yes` is supplied. A dry-run returns `dry_run: true`, profile identity, `before`, `canvas_id`, and the exact one-element `changes` array without calling `canvases.edit`. The preview includes the full current document, requested operation, explicit section ID when applicable, and new content. It is not a section-level diff: Slack's section lookup does not return section bodies.

Interactive writes preview on stderr and default to no. Non-interactive or JSON writes require both `--profile` and `--yes`. Success returns `canvas_id` and the acknowledged `changes`, without automatically rereading or overwriting the canvas.

Slack validates whether the section exists in the target canvas. The public edit API provides no version or compare-and-swap precondition. Content and section IDs can change between inspection and application; preview does not lock the canvas, prevent concurrent replacement, or provide automatic rollback.

## Permissions and errors

Reads and previews require `canvases:read`; edits additionally require `canvases:write` and write access to the target canvas. The scope permits actions beyond the limited command set, so review the actual Slack authorization. Workspace policy, disabled Canvas, resource access, and plan restrictions still apply.

No request is automatically retried, including `canvas_editing_locked` and HTTP 429. Explicit errors such as `canvas_not_found`, `missing_scope`, plan restrictions, and `use_skills_api` are reported without changing API families or scopes. A timeout or invalid/ambiguous write acknowledgement returns `write_outcome_unknown`; inspect Slack before repeating an edit. Read failures remain read failures, not uncertain-write errors.

Local contract tests cover request formats, identity mismatches, explicit sections, previews, UTF-8 size boundaries, safe output, permissions, rate limiting, and uncertain acknowledgements. They do not establish live-workspace acceptance.
