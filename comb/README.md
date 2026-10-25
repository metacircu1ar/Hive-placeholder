<div align="center">
  <img src="logo.png" alt="Comb logo" width="480">
</div>

<h1 align="center">Comb</h1>

Shared Markdown knowledge for people and agents. Comb stores summaries and
append-only evidence, indexes text and links, and records who wrote or reviewed
it.

This guide covers storage and page contracts, text and vector retrieval, the
shared socket service, MCP tools, Hive integration, collectors and ordinary-agent
curation.

For installation and user services, start with the
[setup guide](../docs/setup.md). A standalone text-search setup is:

```sh
comb --root "$HOME/Comb" setup
comb --root "$HOME/Comb" service status
```

Comb computes no embeddings itself. Semantic search uses an HTTP embedding
endpoint: pass `--embedding-config FILE` to setup, or select one later with
`comb model use-http`. `comb model use none` returns to text search. Setup
keeps an existing selection and never waits for background semantic indexing.
An existing root without saved settings, such as one created by `comb init`,
needs one of those explicit choices first.

Setup validates conflicting options before inspecting knowledge or services:
bridge options require `--for-hive`, and service options cannot accompany
`--no-services`. Explicit HTTP configuration is parsed once and retained through
service preparation and application. Invalid configuration is reported against
`--embedding-config`; damaged existing state remains a runtime error.
For setup or service installation, an explicit `--service-path` must contain
colon-separated absolute existing directories, without empty entries or control
characters. Invalid explicit entries are rejected; inherited PATH is filtered.

For example, configure a local HTTP embedding server with:

```sh
comb --root "$HOME/Comb" setup --embedding-config ./embeddings.yaml --no-services
```

Direct `comb setup` also accepts a single-consumer pipe, with absolute asset
paths in the configuration. `hivectl setup --embedding-config FILE` requires a regular
file (symlinks to regular files are supported), because it invokes Comb separately
for preview and apply. Keep that file unchanged during Hive setup.

For the manual examples below, select and initialize a knowledge root:

```sh
COMB="$HOME/project-knowledge"
comb init "$COMB"
```

Use `comb --root "$COMB" COMMAND` below. Uppercase placeholders such as
`HASH`, `UUID` and `NOTE_HASH` mean values returned by the preceding command;
replace example file paths with your own files. Pass `--help` for individual options.

Runtime results are JSON; runtime errors have `error.code`, `error.message` and
`error.file_committed`; mutation errors also return their `request_id`. A true `file_committed` means a durable write may have
happened: reopen to recover, then retry with the same request ID and input.
Request IDs are scoped to the stable caller principal and one exact operation; two agents can independently use the same ID. A successful replay
returns `replayed: true`; page-producing operations return the page's current hash,
while deletion returns the recorded removal result.

Generic filesystem, SQLite and JSON/YAML errors report a safe classification
and parsing coordinates where available. Explicit CLI diagnostics include detailed
causes on the local owner's stderr, never socket or MCP replies.

`comb serve` writes structured diagnostics to `.comb/logs/comb.jsonl` inside
the knowledge root. It captures server events and warnings/errors from Rust libraries using `tracing`
or `log`, retaining
safe error classifications and numeric details while filtering private payloads.
Expected request rejections such as missing pages or conflicting edits are debug
events, excluded at the default info level; operational failures remain errors.
The queue holds at most 1,024 records of 8 KiB each; overflow is counted without
blocking requests. Files rotate at 10 MiB, keeping five files in total. Startup
recovers incomplete records, and graceful shutdown gives logging one second to
drain. A logging startup failure prevents the server from advertising readiness.
Errors before server preparation still use the CLI's bounded stderr diagnostic
(eight causes and 8 KiB, with control characters escaped).

## Command help and parsing errors

Every public command has `--help` with examples. Help and `--version` exit 0
without opening knowledge, reading credentials or contacting a service.
Command-line parsing errors exit 2, with JSON on stdout containing `code`,
`message`, `field`, `expected` and `examples` inside `error`; stderr gives the
human explanation and the same examples. Received values never appear in JSON.
Only explicitly public numeric settings may appear in parsing diagnostics,
escaped and bounded. Payloads, query text and credentials are omitted. Local
filesystem failures can identify the selected path or an unsafe socket ancestor
on stderr, with an escaped permission remedy; JSON and wire replies omit them. Output
write/flush failures exit 1. A managed service whose deterministic startup
failure was successfully recorded keeps exit 0 even if its output fails, to
preserve restart suppression. Once `mcp` starts, stdout contains only protocol
frames. Fatal MCP runtime errors print only a public error code to stderr.

Write-payload errors use the same structured contract. For example, a blank
capture reports `TEXT` in the CLI and `/arguments/text` in an MCP tool reply;
malformed put content identifies `--file` or stdin, and `/arguments/markdown` in
MCP. Socket replies use `/operation/` coordinates. Evidence, source, review-page
and consolidation errors identify the failed field without echoing its contents.
Updating a summary requires the current `--expected-hash` (or `expected_hash`
tool argument), even when the new summary is empty. Authorization failures,
stored-page corruption and conflicting edits remain operational failures. Keep
the returned request ID when retrying an uncertain write with identical inputs.

`comb version` prints the product, version and compiled features as human text;
`comb version --json` prints the same identity as a JSON object. Both work
without a knowledge root, home directory or running service.

Global options can appear before or after the command. An explicitly supplied
option is rejected if the command does not use it:

| Option | Applicable commands |
| --- | --- |
| `--root DIR` | All public commands except `version` and `mcp`. For `init`, use this or positional `DIR`, not both. |
| `--socket PATH` | `setup`, `serve`, `mcp`, and remote knowledge/credential commands, including `doctor`, `stats` and `sync` without `--rebuild`. Local `init`, `upgrade`, `embed`, `sync --rebuild`, `model`, `service` and `version` reject it. |
| `--run NAME` | `put`, `append`, `capture`, `promote`, `consolidate` and `decide`. Required for `consolidate`. |

Most examples use the current directory unless `--root` is supplied. Setup and
service commands instead default to `$HOME/Comb`; pass `--root` to select which
knowledge root they operate on. Replace `HASH`,
`NOTE_HASH` and `TARGET_HASH` with current hashes from `get`, and replace example
files, slugs, identities and endpoint names with your own values. These examples
show command syntax and do not imply that resources already exist.

| Command | Complete example |
| --- | --- |
| `comb` | `comb --help` |
| `service` | `comb service status` |
| `service install` | `comb service install` |
| `service start` | `comb service start` |
| `service stop` | `comb service stop` |
| `service status` | `comb service status` |
| `service uninstall` | `comb service uninstall` |
| `setup` | `comb setup --no-services` |
| `version` | `comb version --json` |
| `model` | `comb model use none` |
| `model use` | `comb model use none` |
| `model use-http` | `comb model use-http --url http://127.0.0.1:8081/v1/embeddings --model embedding-model --dimensions 1024` |
| `serve` | `comb --root /private/knowledge serve` |
| `mcp` | `comb mcp --credential /private/client.json` |
| `credential` | `comb credential list` |
| `credential create` | `comb credential create reader --out /private/client.json` |
| `credential list` | `comb credential list` |
| `credential revoke` | `comb credential revoke 00000000-0000-4000-8000-000000000001` |
| `init` | `comb init /private/knowledge` |
| `get` | `comb get components/writer` |
| `put` | `comb put components/writer --file writer.md --request-id create-writer` |
| `append` | `comb append components/writer Verified --source commit:abc123 --request-id evidence-1` |
| `capture` | `comb capture Investigate --request-id note-1` |
| `promote` | `comb promote components/writer --expected-hash aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa` |
| `consolidate` | `comb --run curate-1 consolidate inbox/note components/writer --note-hash aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa --target-hash bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb --summary-file summary.md` |
| `decide` | `comb decide review/proposal accept --reason Verified --expected-hash aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa` |
| `rm` | `comb rm components/obsolete --expected-hash aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa --request-id remove-1` |
| `list` | `comb list --prefix components/ --limit 20` |
| `search` | `comb search WriterLoop --mode text --limit 10` |
| `embed` | `comb embed --config embeddings.yaml` |
| `links` | `comb links components/writer --limit 20` |
| `backlinks` | `comb backlinks components/writer --limit 20` |
| `graph` | `comb graph components/writer --depth 2` |
| `import` | `comb import ./notes --prefix operations` |
| `sync` | `comb sync --rebuild` |
| `upgrade` | `comb upgrade --dry-run` |
| `doctor` | `comb doctor` |
| `stats` | `comb stats` |

## Pages and evidence

Create `writer.md` outside the Comb directory:

```markdown
---
title: Transaction writer
type: component
tags: [rust, sqlite]
aliases: [WriterLoop, hub writer]
---
The writer serializes durable changes.
```

```sh
comb --root "$COMB" put components/writer --file writer.md --request-id create-writer
comb --root "$COMB" get components/writer
```

`put` also reads standard input when `--file` is absent. Creation requires the
page to be absent. To replace an existing summary, copy the current Markdown from
`get`, retain its evidence entries, and pass its `hash` as `--expected-hash aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa`.
A stale hash returns `conflict`. Existing timeline entries cannot be removed or
rewritten; add correcting evidence instead.

```sh
comb --root "$COMB" append components/writer \
  'The writer rejects empty page titles before saving.' \
  --source 'commit:abc123; issue:empty-titles' --request-id title-check-1
```

When the evidence changes the summary, write the revised summary to `summary.md`
and append both in one operation:

```sh
comb --root "$COMB" append components/writer \
  'Title checks now apply to imported pages as well.' \
  --source 'commit:def456; issue:import-titles' --summary-file summary.md \
  --expected-hash aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa --request-id title-check-2
```

Use `[evidence:new]` in that summary to cite the entry being appended. Comb
replaces it with the assigned evidence UUID atomically. Later edits use
`[evidence:UUID]`. `doctor` checks these references and flags uncited paragraphs;
it cannot establish whether evidence supports a claim.

Timeline metadata is generated by Comb in reserved `comb-entry` comments.
Keep Markdown fences closed. Source text is an assertion, not authentication:
claiming `human` in `--source` never changes the recorded actor.

Human-written summaries/evidence and explicit imports are reviewed; agent writes start
unverified. Appending agent evidence preserves a reviewed summary, but the new
evidence remains unverified. A human append reviews only the new evidence (and a supplied replacement summary); it never reviews untouched agent material. Search reports both matched-chunk `unverified` and
`page_unverified`. A human or configured reviewer can review a full current page:

```sh
comb --root "$COMB" promote components/writer --expected-hash aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
```

## Core command reference

| Command | Purpose and example |
| --- | --- |
| `init [DIR]` | Create private administration state and a text index. Existing valid Comb roots can be reopened. |
| `put SLUG [--file FILE] [--expected-hash aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa] [--request-id ID]` | Create a page or replace its summary using a current hash. |
| `get SLUG` | Return Markdown, structured evidence, trust and the current hash. |
| `append SLUG TEXT --source SOURCE [--summary-file FILE] [--expected-hash aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa] [--request-id ID]` | Append evidence, optionally with an atomic revised summary. |
| `capture TEXT [--request-id ID]` | Create an inbox note, for example `capture 'Investigate writer stalls'`. Its slug hashes the caller principal and request ID. |
| `consolidate NOTE TARGET --note-hash aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa --target-hash bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb --summary-file FILE [--recopy] [--request-id ID]` | Fold an inbox note into an existing ordinary page. Requires global `--run NAME`; copies uncopied evidence and replaces the summary, then checkpoints the note. After inspecting a history conflict, `--recopy` explicitly records a fresh full copy. |
| `decide SLUG accept\|reject --reason TEXT --expected-hash aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa [--request-id ID]` | Record an explicit human/reviewer decision on a `review/` proposal. Does not merge or delete pages. |
| `promote SLUG --expected-hash aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa [--request-id ID]` | Record explicit review of the current summary and evidence. |
| `rm SLUG --expected-hash aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa [--request-id ID]` | Human-only removal of a page and its derived index data. Incoming links become dangling. Reuse the same request ID and hash to retry a deletion safely, even if the page is later recreated. |
| `list [--type TYPE] [--tag TAG] [--prefix PREFIX] [--unverified] [--after SLUG] [--limit N]` | Filter and paginate, for example `list --prefix components/ --tag rust --limit 20`. Limits: 1–1,000; default 100. |
| `search QUERY [--mode MODE] [--embedding-config FILE] [--limit N] [--type TYPE] [--tag TAG] [--prefix PREFIX] [--after RFC3339] [--before RFC3339]` | Hybrid retrieval by default, with explicit text fallback without an embedder; word/identity search uses `--mode text`. For example `search WriterLoop --tag rust`. Limits: 1–100; default 10. Explicit date bounds disable recency boosts. |
| `links SLUG [--after SLUG] [--limit N]` | Outgoing links, for example `links components/writer --limit 20`. Default 100, maximum 1,000; continue after the last returned slug. |
| `backlinks SLUG [--after SLUG] [--limit N]` | Incoming links, for example `backlinks concepts/wal`. Same pagination as links. |
| `graph SLUG [--depth N]` | Undirected neighborhood, including the origin; default 2, maximum depth 8 and 1,000 nodes. |
| `import DIR [--prefix PREFIX]` | Explicitly import Markdown as human-reviewed material, for example `import ./docs --prefix operations`. Import destinations and link references are lowercased; colliding input names are rejected before writing. Hidden files are skipped; symlinks are rejected. Imports commit page by page and retain their sources. Each invocation accepts at most 1,000 pages and 16 MiB of input; select smaller directories for larger imports. |
| `import --status` | Show the latest import's ID, state and confirmed completed/unfinished counts, or `current: null`. Does not need its source directory. |
| `import --abandon BATCH_ID` | End the exact pending batch while preserving completed pages, receipts and source files. Copy its UUID from `import --status`; retries with that ID are safe, and an old ID cannot abandon a newer batch. |
| `sync [--rebuild]` | Reconcile external edits/deletions. Rebuild recreates the derived text index while retaining trust, write receipts and valid cached vectors. It also replaces an unsupported or explicitly corrupt vector database; ambiguous integrity or I/O failures stop the rebuild without discarding that cache. See [ownership and recovery](#ownership-and-recovery) for manual cache disposal. Reactivate the same embedding configuration to reuse unchanged vectors. |
| `upgrade [--dry-run]` | Validate an existing root under exclusive ownership and list planned steps, migration history and retained backups. Stop its service first. No released predecessor exists yet: current roots report no steps and incompatible development data is refused. Never initializes a missing root. See [compatibility and upgrades](../docs/compatibility.md). |
| `embed --config FILE [--replace] [--retry-failed]` | Activate and backfill embeddings; changing the active configuration requires `--replace`. Omit it on first activation or when the same space is already active. Retry permanent input rejects after fixing their cause. |
| `doctor` | Report embedding coverage/rejected chunks, inbox, unresolved-proposal and unverified backlog, orphan pages, broken links, unfamiliar filing prefixes and structural citation problems. All findings share a 1,024-entry cap, including private-state warnings; `findings_truncated` reports omitted findings. |
| `stats` | Return page, chunk, link, pending inbox, unresolved-proposal, unverified and `quarantined_pages` counts. `doctor` lists malformed pages excluded from search. |

An import preflights every page before accepting the batch, then commits pages
individually. Its success report contains `batch_id`, `total`, `completed` (slugs)
and `state: completed`. Repeating the same directory, prefix and unchanged inputs
resumes with the original request IDs and expected page hashes. Completed pages
are skipped even if subsequently edited or removed. A changed unfinished target
stops the retry before any remaining page is written; independent edits are kept.
Import destinations must be outside the source directory.

Comb retains one current or latest finished batch. A matching completed retry
returns its original report without rewriting pages. Once a different batch is
accepted, an older import is a new operation. A partial operation error retains its
error code and nonzero exit, with `import_progress` describing confirmed pages and
any unresolved page; `file_committed: true` can mean the batch was accepted even
before the first page completed. Input rejected by the CLI before contacting Comb
has no batch progress. Opening Comb recovers an interrupted page, but does not
automatically resume the rest of its import.

```sh
comb --root "$COMB" import ./notes --prefix operations
# After interruption, recover the page and discover the retained batch:
comb --root "$COMB" import --status
# Restore unchanged source input and repeat the original import to resume.
# Or explicitly stop that batch using the ID returned by status:
comb --root "$COMB" import --abandon 12345678-1234-4234-8234-123456789abc
```

Status and abandonment still work when the source has disappeared. Abandonment
keeps completed pages and receipts; it does not roll back edits. A new import then
receives a fresh preflight. `DIR`/`--prefix`, `--status` and `--abandon` are separate
modes. Import administration requires human authority and has no agent tool.

A deletion returns `removed`, `request_id` and `replayed`. Save the request ID
before submitting, or retain the generated ID from the reply. For example:

```sh
comb --root "$COMB" rm concepts/obsolete --expected-hash aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa --request-id remove-obsolete
# If the reply is lost, repeat the exact command with the same ID and hash.
comb --root "$COMB" rm concepts/obsolete --expected-hash aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa --request-id remove-obsolete
```

A matching retry acknowledges the recorded deletion without reading or modifying
a page later created at that slug. Changing the slug or hash while reusing the ID
returns `request_conflict`. After an interrupted write, recovery must finish before
a receipt can be acknowledged. Receipts for earlier page-producing operations
also remain: replaying an old `put` or `capture` while its page is absent returns
`result_removed`. Agents have no deletion tool.

Slugs use slash-separated lowercase ASCII letters, digits, `-` and `_`. Suggested prefixes:
`components`, `decisions`, `incidents`, `concepts`, `people`, `operations`, `inbox`,
`review`, `reports`. Pages are bounded to 1 MiB; `doctor` warns at 75%. Put further evidence in a linked page before reaching the cap. Slug links such as
`[[concepts/wal]]` are Comb-root-relative. Markdown links resolve relative to the
page, including paths containing slashes. Use slug links for canonical Comb slugs. Code examples do not
create links.

Search returns one best chunk per page, its heading/zone, a snippet and literal
query highlights as UTF-8 byte ranges. `evidence` explains the matching route.
`create_safety: exists` requires an exact slug, title or alias match; substring
and word matches mean `probable`. Duplicate aliases may identify several existing
pages. The result is evidence to inspect before creating a duplicate, not a
uniqueness constraint. A semantic-only match has `create_safety: unknown`.

## Embeddings

Fresh `comb setup` uses text search unless it is given an embedding
configuration; existing selections are preserved. Embeddings come from an
OpenAI-compatible HTTP endpoint, so the model runs wherever that server runs.
A local GPU option is **Qwen3-Embedding-8B Q8_0**, served by llama.cpp on the
Apple GPU. It needs no API key or paid API, and Comb downloads no models.
The model server owns tokenization and last-token pooling. Comb stores and
searches normalized 4,096-dimensional vectors.

On an Apple Silicon Mac, install llama.cpp and download the pinned model. This
needs about 8.05 GB for the weights, plus memory for inference:

```sh
brew install llama.cpp
mkdir -p "$HOME/Models/qwen3-embedding-8b"
MODEL="$HOME/Models/qwen3-embedding-8b/Qwen3-Embedding-8B-Q8_0.gguf"
curl --fail --location --continue-at - --output "$MODEL" \
  https://huggingface.co/Qwen/Qwen3-Embedding-8B-GGUF/resolve/69d0e58a13e463cd99a9b83e3f5fee7c10265fab/Qwen3-Embedding-8B-Q8_0.gguf
printf '%s  %s\n' d20ddc71e8a5c4344f2343481e242233a997dc5eaff442427a945836c97b4deb "$MODEL" | shasum -a 256 -c -
```

After the checksum succeeds, start the server in that terminal:

```sh
llama-server --model "$MODEL" --alias qwen3-embedding-8b \
  --embedding --pooling last --embd-normalize 2 --device MTL0 --n-gpu-layers all \
  --ctx-size 32768 --parallel 4 --no-kv-unified --batch-size 8192 --ubatch-size 8192 \
  --cache-ram 0 --host 127.0.0.1 --port 8081 --no-webui --no-context-shift
```

It runs in the foreground; Ctrl-C stops it. Metal must be available as device
`MTL0`. Use the same port in the server command and Comb configuration.

The shipped configuration is [examples/qwen3-embedding.json](../examples/qwen3-embedding.json):

```yaml
backend: http
model: qwen3-embedding-8b
revision: q8_0-d20ddc71e8a5c4344f2343481e242233a997dc5eaff442427a945836c97b4deb
url: http://127.0.0.1:8081/v1/embeddings
dimensions: 4096
query_prefix: "Instruct: Given a question about our project, retrieve the knowledge pages that answer it\nQuery: "
document_prefix: ""
timeout_seconds: 60
```

Save the YAML above as `qwen.yaml`. In another terminal, enable it for your Comb:

```sh
comb --root "$COMB" serve --embedding-config qwen.yaml --replace-embeddings
```

For an existing space from another model, `--replace-embeddings` retires the old
vectors and backfills the same pages with Qwen. Pages and evidence are unchanged;
results never mix models. Omit the replacement flag on normal restarts. Offline
replacement is `comb --root "$COMB" embed --config CONFIG --replace`.
Text-only Comb remains usable without configuring a model server.

The server command above configures four slots with an 8,192-token context each, below Qwen's
native 32K limit. Inputs must fit within a slot, including special tokens;
longer inputs fail rather than being silently truncated. An 8B model still needs
substantial unified memory beyond its 8.05 GB weights. Server and Comb memory
are separate costs; monitor both processes when sizing a deployment.

Search remains available during background indexing. The embedding server and
GPU are shared, so large inputs or indexing work can delay semantic queries.
Text search remains available without waiting for embeddings.

Model and server documentation: [official Qwen GGUF](https://huggingface.co/Qwen/Qwen3-Embedding-8B-GGUF)
and [llama.cpp server](https://github.com/ggml-org/llama.cpp/tree/master/tools/server).

### Endpoint configuration

Any OpenAI-compatible embeddings endpoint can be configured:

```yaml
backend: http
model: your-embedding-model
revision: immutable-deployment-revision
url: https://embeddings.example.com/v1/embeddings
dimensions: 384
document_prefix: ""
query_prefix: ""
api_key_env: COMB_EMBEDDING_KEY
timeout_seconds: 30
```

`model use none` disables embeddings; `none` is the only name it accepts.
`model use-http` checks scalar options before reading saved settings or contacting
a service. Dimensions are 1–4096; model and revision are 1–4096 UTF-8 bytes;
query/document prefixes allow empty text and at most 1024 UTF-8 bytes each.
`--timeout-seconds` is 1–120 (default 60 for this command). Supply `--tokenizer`
and `--max-tokens` together. Invalid options name the flag, include an example,
and exit 2. Asset-loading, credential and provider failures remain operational.

`url` is the complete endpoint. HTTP is allowed for loopback endpoints; otherwise
use HTTPS. User-info, query parameters and fragments are forbidden; the normalized
`http:URL` embedding identity must fit within 4096 bytes. Credentials come from the named environment variable, never URL query
parameters. The endpoint must support float responses and the requested dimensions.
Its operator must change `revision` when replacing the model behind an alias;
Comb cannot detect a silent upstream replacement. Requests have a 1–120 second
deadline (default 30), no redirects, 32-input batch maximum and 8 MiB reply cap.
The request omits the optional `dimensions` parameter unless
`request_dimensions: true` is set. The configured width is always validated in
responses. Optionally set `tokenizer: /path/to/tokenizer.json` and `max_tokens`
together (16–8,192) to reject oversized composed inputs locally before HTTP. This
includes the query/document prefix. The endpoint remains the final authority and
still gets rejection isolation when it has stricter limits. Tokenizer bytes,
limit and `request_dimensions` participate in the space fingerprint. The tokenizer
must be a readable regular file of at most 64 MiB; symlinks to regular files and
shared readable files are supported. Pipes, devices and directories are rejected
before reading.

```sh
comb --root "$COMB" embed --config embeddings.yaml
comb --root "$COMB" search 'why are commits slow' --embedding-config embeddings.yaml
comb --root "$COMB" search 'writer' --mode text
comb --root "$COMB" search 'synchronous storage' --mode vector --embedding-config embeddings.yaml
```

The cache identity includes the endpoint's model and revision, dimensions,
prefixes, tokenizer bytes and limit, and normalization. `embed --replace`
retires an old space immediately; results never mix models. Valid cached vectors and
the active selection survive `sync --rebuild` when that selection can be validated.
Missing chunks alone require inference. Old-space cache entries remain reusable;
there is no automatic cache retention policy yet.

Search returns one best chunk per page. Hybrid mode combines text and semantic
ranks. Each stored vector uses four bytes per dimension before database overhead.
The shared service's vector cache defaults to 512 MiB, including estimated entry
overhead; use `serve --vector-cache-mib` to change that budget. It is not a limit
on the whole process or embedding model. Search headings are capped at 1,024
characters; `get` returns the full heading.

Results include `coverage`: `total`, `embedded`, `pending`, `rejected`, measured
after filters. Partial indexing keeps available semantic results; the hybrid text
arm can find pages still pending. Hybrid reports `fallback` when vector retrieval
is unavailable. Vector-only errors if the filtered corpus has no usable vectors;
an empty filtered corpus returns no results. Neither mode silently switches models.

Composed inputs include prefix, title, heading and text. They are limited to 64 KiB
and, with a configured tokenizer, its `max_tokens`.

A chunk the model rejects as too large is embedded in pieces instead:
- **Splitting:** its text is halved, with overlap, at most three times (up to
  eight pieces). Each piece repeats the title and heading.
- **The vector:** the length-weighted mean of the pieces' vectors.
- **Unchanged:** chunks that fit, and their vectors.
- **Still rejected:** a chunk whose pieces cannot fit, for example because its
  title and heading alone are too long. It is rejected without truncation, and
  other chunks continue. `doctor` reports it and `embed --retry-failed` retries
  it.
HTTP 400/413/422 rejections are isolated by splitting the batch within one total
timeout. A minimal-input probe distinguishes endpoint/configuration failures:
if that also fails, work stays pending with `embedding_endpoint_rejected`. Only
when the probe succeeds does an individually rejected input become an
`embedding_input_rejected` finding; other inputs and later batches continue. Correct the content or endpoint
configuration, then retry failed inputs. Authentication, URL, network and server
failures remain retryable without marking individual chunks rejected. While serving,
`doctor.embedding_worker_error` shows the latest worker failure until a successful
publication; it resets at service restart.

## Shared serving and MCP

```sh
# One long-running process owns this Comb; omission uses its saved model selection.
comb --root "$COMB" serve --embedding-config embeddings.yaml
# Another terminal: ordinary CLI commands automatically use the service.
comb --root "$COMB" search 'why commits stall'
comb --root "$COMB" capture 'Review the latest writer measurements'
```

Explicit embedding config files are read once, with a 16 KiB limit, before
knowledge access. Scalar values and option pairs are checked before managed state,
assets or runtime setup. Errors name the selected flag and the configuration field,
with a command example. Asset and credential failures remain runtime errors.
Relative `tokenizer` paths resolve
against the selected config file's directory, including when that file is a
symlink. Pipe-sourced configs require absolute asset paths. For example:

```sh
comb --root ./knowledge embed --config ./models/embedding.json
comb --root ./knowledge search "project architecture" --embedding-config ./models/embedding.json
```

Invalid explicit query models fail the command; they do not silently turn hybrid
search into text-only fallback. Normal query-time fallback remains available.

`serve` accepts `--embedding-config FILE`, `--replace-embeddings`,
`--retry-failed` and `--vector-cache-mib N` (1–8,192 MiB; default 512 with a model).
The last three options require an explicit embedding config or an endpoint
selected through setup or `model use-http`. Text-only serving rejects them, including an
explicit `--vector-cache-mib 512`, before recovery or index changes. If the
selection changes after the model loads, startup stops; retry with the current
selection. It syncs files before listening, then backfills missing vectors in
bounded batches. Changing
model configuration requires restarting the service; `--replace-embeddings`
explicitly retires the previous active space. Local `init`, offline `embed` and
`sync --rebuild` require exclusive access. Stop the managed service for the same
root with `comb --root "$COMB" service stop`, or stop the process holding it,
before retrying. With a running service, omit search's `--embedding-config`:
the service owns that configuration. If no query model is configured, supply
`--embedding-config FILE` for offline search or configure the serving model.
Use `--mode text` for literal search without an embedding model.
Startup checks the chosen socket before
recovering a pending model activation or syncing pages. An existing socket is
reclaimed only while holding its exclusive lease and after its private ownership
record matches this knowledge root and the exact socket entry. A ready server
can restart automatically after a crash. Connection refusal alone never proves
that a socket is stale: a live server with a full listener queue can also refuse
connections. If ownership evidence is missing or mismatched (including a crash
between binding and recording ownership), startup refuses the socket. Confirm
that its service has stopped before manually removing it, or choose another
`--socket` path. Graceful shutdown removes the service's socket and ownership
record. The persistent `.comb-socket-*.lock` files must not be deleted while
any service or startup attempt might use that socket.

The server binds only inside a directory owned by your account with no group or
other permissions (normally `0700`). This keeps the socket private from the
moment it is created, including under a permissive umask; its final mode is
`0600`. Custom socket directories must have safe ancestors: owned by your account
or root. Group-writable ancestors owned by your non-root account are allowed,
including Ubuntu/Debian user-private-group directories created with umask `002`
(mode `0775`). This trusts your group's members to modify ancestor entries; Comb
does not check group membership. World-writable ancestors are refused, and
root-owned ancestors must not be group-writable either, except for root-owned
sticky directories such as `/tmp`. Comb never changes existing directory
permissions for you. Existing user-owned knowledge roots, page subdirectories
and credential-output parents allow the same group-write permission;
Comb's private administration directories and regular-file checks remain strict.
Safe search-only ancestors are
supported; directories that setup must modify need read access for durability checks.
The socket's immediate parent must still be private. Unsafe directories
are rejected before embedding setup or recovery. Comb does not change permissions
on an existing public directory. Comb setup can create missing socket
directories privately; `serve` requires them to exist.

For example, create a short private path and use the same override on clients:

```sh
mkdir -m 700 "$HOME/comb-socket"
comb --root "$COMB" --socket "$HOME/comb-socket/service.sock" serve
# In another terminal:
comb --root "$COMB" --socket "$HOME/comb-socket/service.sock" stats
```

Without `--embedding-config`, serving uses the selection saved by setup or
`model use-http`; it is text-only when no endpoint is selected. To disable embeddings
deliberately, stop the service and run `comb --root "$COMB" model use none`
before starting it again.

Use global `--socket /absolute/private-directory/comb.sock` on the server and
CLI if the default `.comb/service.sock` exceeds the Unix socket path limit.
Keep the directory private. CLI offline fallback is allowed only when connecting
fails with not-found/refused **before submission**. A timeout or lost reply never
starts another writer. Retry put/append/capture/promote with the same request ID
and exact input after an `uncertain` response. A client deadline does not cancel
accepted work. Reads use committed indexed snapshots; after hand edits run `sync`.
A mutation reconciles its own target before updating it. If a later external edit
causes `conflict`, run `sync` before `get` to index that edit and obtain its current
hash for a retry.

Semantic queries can return `embeddings_busy` when capacity is full or inference
cannot start within one second. Hybrid mode reports an explicit text fallback;
vector-only mode returns the error. A slow embedding request cannot be interrupted
by another query. Text reads and writes remain available during inference.
If durable publication fails, new reads are blocked until restart and recovery.
A panic while mutating shared writer, model, cache, publication or credential state
also fences that Comb. New requests return `recovery_required`; restart the
`comb serve` process to recover. A machine reboot is unnecessary. Already
pinned reads may finish from healthy data, but further shared-lock access fails.
Hybrid search propagates this error instead of hiding it with text fallback.
Socket requests are capped at 8 MiB, replies at 32 MiB, and MCP output at 64 MiB to
cover escaping of valid 1 MiB pages. Service requests have a 60-second deadline.

Issue private credentials using the human CLI. Names are for administration;
server-generated IDs bind identity and durable retry receipts.

```sh
comb --root "$COMB" credential create editor --out /private/path/editor.json
comb --root "$COMB" credential create reviewer --reviewer --out /private/path/reviewer.json
comb --root "$COMB" credential create hivectl --bridge \
  --reviewer-role reviewer --out /private/path/bridge.json
comb --root "$COMB" credential list
comb --root "$COMB" credential revoke CREDENTIAL_ID
comb mcp --credential /private/path/editor.json
```

Create the private output directory first. Credential files contain secrets and
must remain mode 0600; they are never printed by `credential create`. Do not put
administrator or bridge credentials in agent workspaces. A standalone client
has a fixed client identity; `--reviewer` grants explicit promotion capability.
Bridge credentials attest agent/role/execution/turn or collector identity, with
promotion limited to repeated `--reviewer-role ROLE` values. They cannot import,
delete pages or administer credentials. Only already-admitted requests may finish
after revocation. Lost/removed credential registry data admits no old client keys.
An uncertain credential update fails closed until the service restarts.

Credential outputs, setup bridge outputs and custom server socket names cannot
use `.hive-tmp-state-` or `.comb-tmp-admin-` followed by exactly six ASCII
letters/digits or sixteen lowercase hexadecimal digits. Those basenames are
reserved for Hive state and Comb administration preparations. This restriction
does not apply to input files or attachment basenames.

After successful store admission and pending recovery, Comb removes abandoned
`.comb-tmp-admin-` preparations from `.comb/`, `.comb/heads/` and
`.comb/receipts/`, without descending into other directories. Cleanup holds the
store writer lock and refuses unsafe entries or changed ownership; a failure
stops admission. Correct the reported filesystem or ownership problem and retry.
Ordinary pages, credentials, authoritative records and nested stores are retained.
Page writes prepare complete private files in their own `.comb/` directory and
atomically rename them into place on the same filesystem. Success includes syncing
both directories; recovery repeats those barriers before clearing a pending intent.
Abandoned page preparations are collected on successful reopen. Arbitrary credential
outputs, protected initialization stages, setup/provisioning temporary files and
unrecognized generic temporary files are retained. Cleanup never infers ownership
from a file's age or recursively sweeps page directories.

`credential create` checks `--out` and prepares private temporary storage before
issuing a credential. Existing files and symlinks are never overwritten. The final
file appears only when its complete contents are ready. If saving fails before
publication, the CLI attempts to revoke the issued ID and reports whether that
revocation was confirmed. If the file was already published when a durability
check failed, it remains active and the error carries `file_committed: true`.

A lost reply or a process crash between issuance and publication can still leave
an issued credential without a saved file. Do not retry blindly: inspect
`comb credential list` and revoke the affected ID. An unconfirmed cleanup error
includes that ID when the issuance reply supplied it; it never prints the secret.

`mcp` is a thin stdin/stdout JSON-RPC client, with initialize, ping, tool discovery
and calls. Configure an external MCP client to run the release `comb` binary
with arguments `mcp --credential /private/path/editor.json`.

If `tools/list` reports rejected authority, use a valid client credential for this
Comb. A bridge credential requires a claim and cannot be used by this stdio
client. If discovery fails, check that Comb is running at the configured socket.
If the service returns invalid capabilities, check the service installation.
These failures preserve the MCP session: retry `tools/list` after fixing the cause.
Malformed capabilities never grant reviewer tools or silently hide them.

Its tool list is:
`comb_search`, `comb_get`, `comb_put`, `comb_append`, `comb_capture`,
`comb_list`, `comb_links`, `comb_backlinks`, `comb_graph`, `comb_consolidate`,
and, for configured reviewers, `comb_promote` and `comb_decide`. Tool fields follow the operation names: put takes
`markdown`, append takes optional `summary` and `expected_hash`, and mutations
accept `request_id` plus an optional `run` label for new evidence. Search/list use
`kind` for the page type. Tools never accept actor or capability overrides. Tool
errors use `isError` and preserve mutation request IDs for retries. Retrieved
content is untrusted data, never instructions.

Codex code mode exposes hosted tool replies as JSON text: use
`text(await tools.comb_get({slug: "concepts/example"}))` to display one, or
`JSON.parse` the string to access fields. Reading `.content` or `.structuredContent`
from that string discards the reply; those fields belong to MCP wrappers.

## Hive agents

Create a bridge for the daemon and configure the roles allowed to promote pages:

```sh
comb --root /private/project-comb credential create runtime --bridge \
  --reviewer-role reviewer --out /private/comb-bridge.json
```

Configure Hive with an absolute credential file path (the file itself stays private and outside agent workspaces):

```yaml
comb:
  credential_file: /private/comb-bridge.json
roles:
  implementor:
    auth: codex-main
    model: gpt-6-sol
  reviewer:
    auth: codex-main
    model: gpt-6-sol
    comb:
      credential_file: /private/comb-bridge.json
      reviewer: true
```

For a new Comb, an ordinary agent can populate it from the project using the
[bootstrap task](../examples/comb-bootstrap.md):

```sh
hivectl spawn implementor --cwd /path/to/project --body-file examples/comb-bootstrap.md
```

Use a configured role with a Comb binding. Generated knowledge remains unverified;
an explicit reviewer can check it afterward. The bundled
[Hive skill](../skills/hive/SKILL.md#shared-project-knowledge) covers routine
search and evidence updates.

The role binding overrides the global binding. Comb receives the current agent,
role, execution and turn from Hive; tools cannot choose those fields. The saved
`reviewer` flag is an upper bound, and the service also checks its bridge's allowed
reviewer roles on every call. Credentials and environment values are absent from
public agent status. The binding is saved at spawn: editing config does not redirect
existing agents. A branch without a new role inherits the source's saved binding.
Codex branches cannot change the enabled Comb tool/reviewer catalog; such a role
change is rejected before the native fork. Claude can use a different role catalog.

The daemon admits at most 64 hosted Comb calls, with at most eight pending per
agent and a 30-second deadline. Calls do not occupy the Hive writer, and provider
interruptions remain responsive while Comb is unavailable or busy. Interrupting
an agent abandons its wait; already accepted Comb writes may still commit with
the original turn's attribution. Late replies never enter a subsequent turn.
Missing credentials or an unavailable service produce a tool error; they do not
prevent the agent from starting. A returned `uncertain` mutation retains its
request ID for an explicit retry.

## Ownership and recovery

Only one writer opens a Comb at a time. Content and administration state must share a filesystem; nested mounts and mis-cased on-disk paths are rejected. Keep the Comb outside hosted agents'
writable workspaces; use authenticated tools for their writes. This is a cooperative same-user policy, not protection
against another process with access to the administrator's files.

Commit the Markdown to Git; **back up `.comb/` privately**. Its nested `.gitignore` excludes administration files, and `doctor` flags missing protection or tracked private state. Never publish the seal key. Page seals bind
content and owned metadata to the Comb identity and slug. The last-written
hash detects restoring an older sealed page. External edits, copied pages and
rollbacks lose claimed authorship, review state and page-owned consolidation metadata; `sync` marks
them external and unverified. Explicit human import or promotion can review them.
The SQLite index can be rebuilt. The identity key, saved heads and replay receipt sidecars are authoritative and must be retained. Replay receipts are keyed by principal and request ID and are not pruned or stored in page frontmatter.

Reopening reconciles interrupted writes. After hand edits, run `sync`; search
and counts otherwise reflect the last indexed state. Ordinary sync quarantines
malformed, invalid-UTF-8 or oversized pages: their bytes remain untouched, stale
search entries are removed, and `doctor` reports them. Repair the page and sync
again to restore indexing. Unsafe paths, ownership failures and I/O errors still
stop scanning; unseen pages are never treated as deleted after a partial scan.
An edited or malformed target of an already accepted write or consolidation still
fences recovery, preserving both the intent and edited bytes.
An unsupported derived text schema is rebuilt automatically only when the vector
cache and saved embedding selection can be preserved. An accepted index rebuild
records the exact owned database files before
removing them. Reopening resumes interrupted deletion and population; it refuses
changed file identities or unsafe files. Native SQLite journals are included.
If offline `embed` reports pending recovery, run `comb sync` with the same root
first, then retry `embed`. Rebuild recovery preserves pages, trust and receipts;
replacing an invalid vector database requires recomputing its embeddings.

Page reads check the open file and its named entry before accepting the bytes.
Changes observed during a read cause up to three attempts; continuing changes
return `conflict`. Updates and deletion retain the hash of the content they loaded
and check it again before accepting a write and before publishing or removing the
file. A detected edit before acceptance returns `conflict` without a successful
receipt; readers remain usable. If an edit is detected after the durable write
intent, Comb preserves it and returns `recovery_required`. Stop the server,
back up the page and `.comb/intent.json`, and resolve the conflicting content
before reopening; recovery will not overwrite an unrelated third state.

New pages are published whole and cannot replace a file another writer created
first. These checks do not make external editing transactional: an in-place editor
can pause on incomplete content, a rewrite can leave identical observed metadata
within the filesystem's timestamp resolution, and an edit can land between the
final check and replacement or deletion. Use Comb's write commands for serialized
changes, or stop Comb while editing Markdown and run `sync` afterward.

If rebuild reports that it cannot verify vector cache integrity, the cache was
preserved: SQLite's integrity diagnostics can also indicate storage read errors.
Check storage health first. To explicitly discard cached embeddings, stop Comb
for that root, then remove only `<root>/.comb/vectors.sqlite` and its
`vectors.sqlite-wal`, `vectors.sqlite-shm` and `vectors.sqlite-journal` companions
(if present). Run `comb --root /absolute/path/to/root sync --rebuild`, then
`embed` with the same embedding configuration to recompute the vectors. Keep all
other files in `.comb/`; they include authoritative identity and replay records.

If Comb reports an incompatible root, preserve the original pages and create a
new root as described in [incompatible state](../docs/platforms-and-installation.md#incompatible-state).

Curation uses an ordinary Hive role and skill, run on demand or from your own cron.

## Evidence collection and curation

Enable collectors explicitly in Hive's global configuration:

```yaml
comb_collectors:
  project:
    credential_file: /absolute/private/comb-bridge.json
    sources: [review, correction, task_completed, turn_failed, approval_answer]
    enabled: true
```

This is independent of hosted `comb` tools. Automatic captures contain only
allowlisted identities and outcomes. Completion prose, prompts, approval answers,
provider error text and artifact contents stay out of automatic evidence. Approval
captures identify the human decision maker and the originating agent/turn.
Deliberately share useful public prose with `hivectl evidence` or
`hive_evidence`; see the [CLI reference](../docs/cli-reference.md#shared-learning-evidence).

At initial configuration, the private credential pins the intended Comb UUID,
even if its service is offline. An unreadable credential leaves the destination
`identity_unbound` and paused until an explicit human `collectors rebind`. A later
Comb UUID change also pauses delivery. Same-Comb key rotation allows fresh items
to continue; attempted items remain pinned to their original principal until
receipt replay, explicit release (which may duplicate), or discard.

Accepted evidence is copied durably into a bounded outbox in the event transaction.
Agent purge and event compaction retain those copies. Up to 16 named destinations
are retained, each with 1,024 items/16 MiB. Overflow preserves accepted work and
records skipped sequence bounds/count; recovered capacity queues a gap notice
before new evidence. Removing/disabling configuration stops new admission while
pending work continues using its saved credential path. Four deliveries can run
concurrently, fairly across destinations, with bounded service waits and retry
backoff; credentials and network I/O stay outside the writer. `hivectl collectors
status` reports pending bytes, uncertain items, gaps and errors. `comb doctor`
reports the already-delivered inbox and proposal backlog.

Writers should update a summary in the same append that changes its evidence.
Run the ordinary [curator skill](../skills/comb-curator/SKILL.md) when the inbox
needs attention, using [the example role](../examples/comb.yaml) and task:

Start a daemon with that configuration, then address the same state directory:

```sh
hivectl --config examples/comb.yaml --state-dir /absolute/private/state daemon start
hivectl --state-dir /absolute/private/state spawn curator --cwd /absolute/workspace --body-file examples/comb-curate.md
```

An external cron may invoke the second command, using absolute paths for the
working directory and task file. Hive has no curator scheduler.
The curator reads each source and target hash, then consolidates:

```sh
comb --root ./knowledge get inbox/NOTE
comb --root ./knowledge get concepts/writer
comb --root ./knowledge --run curate-inbox consolidate inbox/NOTE concepts/writer \
  --note-hash aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa --target-hash bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb --summary-file updated-summary.md \
  --request-id fold-note-1
comb --root ./knowledge decide review/writer-duplicate reject \
  --reason 'These pages describe different responsibilities.' --expected-hash aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
  --request-id decide-duplicate-1
```

The replacement summary can cite `[evidence:new]`. Consolidation prepares the
target update and source checkpoint in one durable recovery record. New Comb
snapshots see both changes together; the two Markdown files are replaced in
sequence. An interrupted accepted change fences further operations until reopen
finishes the exact prepared pages and both receipts. Retry the same request ID and
input after recovery or a lost reply. If either file was independently changed
during interruption, recovery refuses to overwrite it. Preserve the files and
`.comb/intent.json` for inspection and resolve the conflict before continuing.
Authenticated coverage in the target prevents recopying old evidence. Later note
edits reopen its backlog; subsequent consolidation copies only new evidence or
changed summary fields. Copied Markdown stays opaque, including timeline markers.
Summary hand edits remain incremental while either page retains authenticated
coverage of the unchanged evidence. Rewritten/deleted history or loss of all
coverage for a prior copy is a conflict. New trusted notes can be consolidated
into a hand-edited target normally. Inspect both pages, then use `--recopy` with current hashes
and a new request ID to record an explicit full copy if appropriate. Cleared
authorship and review trust remain cleared.

`comb_consolidate` exposes `note`, `target`, `note_hash`, `target_hash`, `summary`,
`run`, `request_id` and optional `recopy`. `comb_decide` exposes `slug`, `expected_hash`, `decision`,
`reason`, optional `run` and `request_id`, and is advertised only to configured
reviewers. Decisions record actor/reason and close the proposal backlog without
silently changing other pages. Editing a decided proposal reopens it.

## Input bounds and validation

The slugs, IDs, retrieval bounds/filters and argument combinations below are
checked before reading Markdown/stdin, opening the root or contacting the service. These input errors exit 2
with JSON on stdout and an explanation and command example on stderr. Caller
text is omitted; rejected public numeric settings appear only on local stderr.

- Slugs and page cursors contain 1–240 bytes in slash-separated components of
  1–100 lowercase ASCII letters, digits, `-` or `_`. Import prefixes are slugs;
  list/search prefixes are literal nonempty strings up to 240 UTF-8 bytes and
  may end in `/`. Type/tag filters contain 1–256 UTF-8 bytes.
- Search queries must be nonblank and fit in 4096 UTF-8 bytes. Search limits are
  1–100; list/link limits are 1–1000; graph depth is 0–8. Serve vector cache
  budgets are 1–8192 MiB. Defaults are unchanged.
- Search dates are RFC3339 timestamps with timezones. `--after` must be at or
  before `--before`, comparing instants across offsets. Text search rejects
  `--embedding-config`; hybrid/vector search can use it for offline queries.
- Request IDs contain 1–128 UTF-8 bytes; run names and credential labels contain
  1–512. These identifiers reject control characters and preserve their spelling.
  Credential revocation requires an issued UUID. Reviewer roles use the same
  512-byte bound, are unique, and have a maximum of 64 in setup and credentials.
- Appending with `--summary-file` requires `--expected-hash`. Consolidation requires
  `--run`. Choose either `init DIR` or `--root DIR init`, not both.

Content hashes are exactly 64 lowercase hexadecimal digits. The repeated `a`/`b`
hashes in command examples illustrate the required shape; replace them with the
current hashes returned by `comb get` before editing existing pages.

Mutation payloads are prepared before opening knowledge or contacting its service.
`capture` and `append` text must be nonblank and fit in 512 KiB; append sources
and decision reasons must be nonblank and fit in 4096 UTF-8 bytes. `decide` requires
a page under `review/`. Consolidation requires a note under `inbox/`, a target
outside `inbox/` and `review/`, and a nonblank replacement summary.

`put --file` and `--summary-file` read at most 1 MiB of UTF-8 text. Put validates
Comb Markdown before execution; summaries cannot contain NUL bytes. An empty
append summary clears the summary. Files may be ordinary shared-readable files
or symlinks to them. Single-consumer pipe inputs must finish within 30 seconds;
stdin remains available for interactive `put` input. Import requires an accessible
directory whose resolved absolute path is UTF-8.

Malformed payloads and selected-file errors exit 2 with the affected field and an
example. Selected paths appear only on local stderr, escaped and bounded; file
contents and parser details are omitted. Resource or storage failures remain
operational errors (exit 1). A rejected payload does not reconcile external edits
or change the knowledge index.

Knowledge commands check `--root` before opening the index or loading a model.
A missing root or missing `.comb` directory identifies the root and suggests
`comb init DIR`; existing unsafe or corrupt administration files remain
operational failures. Configuration parsing and payload checks come first.
Explicit `--socket` paths must name a socket in an accessible existing directory,
with an absolute path of at most 103 bytes; invalid selections identify that flag.
For `serve` and `setup`, the directory privacy rules above also apply. Explicit
unsafe output selections are input errors before managed-service changes; unsafe
saved/default selections or later permission changes are operational failures.

Administrative socket operations report trusted field coordinates for malformed
input, including `/operation/prefix`, `/operation/id` and nested
`/operation/grant/...` fields. Unknown fields are rejected even on zero-argument
operations. The CLI translates these to its own arguments, such as import
`--prefix`, credential create `NAME` or `--reviewer-role`, and credential revoke
`ID`; those input failures exit 2. Grant-name and reviewer-role value checks run
after authorization. Stored-credential corruption and rejected authority remain
operational failures. Rejected input never echoes credential values or unknown
field names.

`comb mcp --credential FILE` reads its credential once before starting the
protocol. The file must be owned, private (0600), regular, at most 16 KiB, and
contain the connection object written by `credential create`; symlinks, positional
arrays and duplicate fields are rejected. Credential errors identify
`--credential`; an invalid socket override identifies `--socket`. Startup input
errors exit 2 and print the code, field, expected form and example only to stderr.
MCP stdout stays reserved for protocol frames. Diagnostics omit credential
contents, private selected paths and native parser details; operational failures
exit 1 with troubleshooting guidance.

Retrieval tools (`comb_search`, `comb_get`, `comb_list`, `comb_links`,
`comb_backlinks` and `comb_graph`) validate arguments before contacting the
service. Search requires a nonblank `query`; page limits and graph depth must be
within the ranges advertised by `tools/list`. Input errors include `code`,
`message`, `field`, `expected` and `examples`. For example, a search with
`{"query":"architecture","limit":0}` identifies `/arguments/limit` and expects
an integer from 1 through 100. Unknown argument keys identify their containing
object without echoing the key or value. Optional filters accept `null`; a null
list limit means the default, while search/link limits and graph depth require
integers when present.

Write tools (`comb_put`, `comb_append`, `comb_capture`,
`comb_consolidate`, `comb_decide` and `comb_promote`) use the same diagnostic
fields for malformed arguments, slugs, hashes, request IDs and run names, before
contacting the service. For example, `comb_capture
{"text":"Check the cache","request_id":null}` identifies `/arguments/request_id`.
Omit `request_id` to use the hosted call's stable identity (or a generated identity
for standalone calls); an explicit ID must be a nonempty string. Optional run,
hash and summary values accept `null` where omission is allowed. Consolidation
requires a non-null run name. Full examples and field descriptions are included
in `tools/list`. Authorization, page-content validation and hash conflicts still
apply when executing a valid request.

Socket read and write errors use `/operation/...` field pointers. The CLI maps valid
pointers to the submitted command's flags or positional arguments and exits 2.
Errors retain `file_committed` and any `request_id`; never assume rejection means
a previously reported write can be retried with a new ID. Malformed or unrelated
reply diagnostics remain operational failures (exit 1).

## Diagnostic logs

The server writes sanitized structured diagnostics to
`ROOT/.comb/logs/comb.jsonl`. It retains five files of at most 10 MiB each by
default. Set `serve --log-level off|error|warn|info|debug|trace`,
`--log-file-mib 1..1024` or `--log-files 2..64` to change those settings.
`COMB_LOG` overrides the level. A malformed value rejects server startup before
recovery; standalone inspection commands warn and use info. Structured logs exclude
page bodies, credentials, selected paths and raw native error messages.

```sh
comb --root /private/knowledge logs --tail 100
comb --root /private/knowledge logs --level warn --tail 20
comb --root /private/knowledge logs --event comb.ready --follow
comb --root /private/knowledge serve --log-level debug --log-file-mib 20 --log-files 3
comb --root /private/knowledge service install --log-level warn --log-file-mib 20 --log-files 3
```

`logs` needs no running server and opens no knowledge database. `--tail` accepts
1..2000 matching records and reads at most 8 MiB per page; `--level` is a minimum
severity, and `--event` matches an exact static event name. `--follow` is for humans:
it prints complete new records until Ctrl-C, warning if rotation pruned its previous
position. Incomplete trailing records are withheld; corrupt fragments are reported
and skipped. Missing roots or logs are never created by inspection.

Standalone CLI diagnostics go to stderr while JSON results remain on stdout. MCP
stdout remains reserved for protocol frames. A disconnected caller does not undo or
replay accepted work; delivery-loss counters describe that loss without retaining the
request. SIGINT and SIGTERM initiate graceful shutdown and record the signal.

`service install` saves these logging options for future managed starts. They are
invalid with service start, stop, status or uninstall. The service's private
environment file may set `COMB_LOG` to override the saved level at startup.

Managed native stdout/stderr is a separate private raw log. It may contain paths
or native messages and is not sanitized or automatically exported. The admitted
process checks it at startup and once per second: above 10 MiB it saves the last
1 MiB to a `.prev` sibling, then truncates the same active inode. This is soft
retention: output can overshoot between checks, while the owner is stuck, or when
startup fails before ownership admission. Concurrent native writes during trimming
can be discarded. A failed tail publication preserves the active file. Foreground
output redirected to an unrelated file is left under the caller's control.

`doctor` reports `private_state_unchecked` when Git tracking cannot be inspected,
including roots outside a Git repository. This is incomplete evidence, not proof
that private state is untracked. Resolve the check and run `doctor` again.
