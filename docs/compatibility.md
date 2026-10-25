# Compatibility and upgrades

Hive currently has pre-release status. There is no compatibility promise between
development builds, even when their product versions are equal. A public tag,
downloadable archive or installer does not change this status. The first supported
release will be explicitly declared and identified here.

## Current data

`hivectl version --json` and `comb version --json` report the product version,
compiled features and checkout commit captured at build time. The commit is
`null` for source archives without Git. A commit identifies the checkout's HEAD;
it does not certify that the source tree was unmodified or identify the exact
executable bytes. Feature suffixes remain visible in `--version` output.

The JSON also reports owned `formats` and `shared_formats`, including each
format's current number, supported range and declared-release fingerprints.
Those release lists are currently empty. State roots have a required `format`
marker; the runtime database also carries its application identity and format.
Comb keeps its root marker in `.comb/format`. Connection, collector, service
and installer records identify their own formats. Matching markers never bypass
the exact structure, ownership or integrity checks.

Keep the old executable, configuration, state directory and Comb root when
changing builds. An incompatible directory is refused; installing a new binary
does not convert it. Select a fresh directory using the
[incompatible-state instructions](platforms-and-installation.md#incompatible-state).
Do not remove or edit format markers to bypass validation.

A missing required marker in an existing directory is also an incompatibility.
Hive does not adopt that directory or reset it automatically. Preserve the entire
Comb root, including `.comb`: its identity, seal key, credentials and receipts
cannot be reconstructed from Markdown alone. Re-adding content to a fresh root
does not carry over the old root's trust, receipts or credentials.

Use `hivectl --state-dir /private/hive-state state upgrade --dry-run` or
`comb --root /private/knowledge upgrade --dry-run` to validate an existing root
and list planned steps, completed migrations and retained backups. Stop every
writer for that root first; inspection requires exclusive ownership. The current
pre-release build has no released predecessor adapters and never converts
development state. A current root reports no steps. Omitting `--dry-run` does
not turn incompatible development data into supported input.

The migration framework retains exact original affected files and a consistent
SQLite backup, validates prepared output, and publishes the format marker last.
Interrupted publication retains a recovery intent. Detected external edits fence
recovery rather than being overwritten; retain the conflicting files and backups.
Do not edit a root during migration. Migration manifests are bounded to 64 MiB
per step; an oversized step is refused before publication of authoritative files.

Comb can automatically rebuild an unsupported derived text index while retaining
its valid vector cache and active embedding selection. This does not authorize
replacing an unknown vector-cache format or damaged authoritative root files.

## Connections and running binaries

Control requests and Comb socket requests name their protocol explicitly. The
current supported range is 1 through 1; there is no invented predecessor adapter.
Unsupported versions are refused before dispatch. `hivectl daemon status` reports
the daemon's product version, commit, frozen binary SHA-256, control range and
catalog number. `daemon start` checks that digest against the selected build;
use `hivectl daemon restart` to replace a running build after orderly cleanup.
Compatible ordinary control commands remain usable without a forced restart.

Every saved agent pins its tool catalog. An unsupported stored catalog is refused
rather than silently rebound to different tools. Native shell helpers come from
the daemon's frozen launch image and carry their own checked protocol number, so
replacing an installed executable cannot change helpers in a running daemon.

MCP initialization echoes a supported requested version, or offers the endpoint's
preferred version; a client that cannot use that version must disconnect. Hive
external MCP and Comb support `2025-11-25`; the native Claude SDK bridge supports
`2024-11-05`. The supported lists are reported by `version --json`.

An unfamiliar Comb error code is exposed as `unknown_error` with its bounded
original value in `raw_code`, while preserving validated `file_committed`, request
ID and import progress. Those fields govern
recovery; an unfamiliar code is never permission to retry a write. A malformed or
lost reply after submission remains uncertain, with the original request ID.

Shared collector and connection records follow expand/contract after release:
first readers learn both released shapes, then writers change, and the old reader
is removed only after the supported previous-release window ends. Each accepted
shape is strict. Queued collector payloads remain as written; credentials and
connection files are never silently rewritten to compensate for incompatible
parts. The current development build accepts only the current marked shape.

## Public interfaces and deprecation

The following Semantic Versioning rules apply from the first explicitly supported
release. They cover the CLI, JSON results, exit/error codes, hosted and MCP tools,
configuration, and bundled skills. They do not create compatibility promises for
the current development state.

| Change | Release rule |
| --- | --- |
| Correct behavior to match the documented contract | Patch; retain existing data formats and protocols |
| Add an optional command, flag, tool or configuration key | Minor; existing invocations retain their meaning |
| Add an output object field | Minor; consumers must ignore unknown fields |
| Remove or rename an accepted command, flag, tool, key or output field, or change its type/meaning | Major, following deprecation |
| Change an existing exit status or error-code meaning | Major |
| Add an error code for new behavior | Minor; consumers must preserve an unfamiliar failure instead of treating it as success |
| Add a response variant that existing consumers cannot safely handle | Major or an explicitly negotiated new protocol |

Scripts should parse JSON fields by name, ignore unknown object fields, and use
the exit status plus documented result/error codes. Do not parse human error
wording, diagnostic logs, field order, whitespace or command-help formatting.
Unknown result variants are not successful replies; retain the complete response
for inspection. Unknown input fields remain invalid. The output extensibility
rule does not authorize sending invented request fields or bypassing strict data
and protocol validation.

Deprecations appear for at least one published release before removal in a later
major release. A deprecation identifies its replacement when one exists, the first
warning release and the planned removal major. Configuration warnings go through
the diagnostic sink; `hivectl config check` and `hivectl daemon status` expose structured
entries. `hivectl config check --fix` applies only explicitly registered safe renames
with a backup. Other changes require manual action; a warning never grants an
automatic migration. The current deprecation table is empty.

Bundled skill frontmatter records `metadata.version`, matching the product version,
and `metadata.hive-agent-catalog`, identifying its hosted tool contract. Distribute
skills with their matching CLI reference. Installing a new guide does not change
an existing agent's pinned tool catalog. Refresh external skill copies through
setup and inspect the reported changes; user-edited copies remain protected.

## Policy from the first supported release

The compatibility promise begins with the explicitly declared release, including
if its product version starts with `0`. Product versions use Semantic Versioning;
data formats and part protocols have separate numbers. Matching product versions
alone do not establish that data is valid: readers also check exact structure,
ownership and integrity.

Newer releases will upgrade data from any earlier supported release, after an
automatic backup. Backups are retained until deliberately removed. Older binaries
will refuse newer data without rewriting it. Patch releases will not change data
format or protocol numbers.

Parts will support their current and previous released protocols. An unsupported
combination will identify the part that needs updating. Installing binaries will
not silently replace a manually running daemon; a mismatch will be reported.
Provider support is reported by `hivectl version --json`, `hivectl auth list`,
`hivectl auth status PROFILE`, `hivectl doctor` and the daemon's runtime status.

| Provider | Minimum | Last tested |
| --- | --- | --- |
| Codex | 0.154.0 | 0.161.0 |
| Claude Code | 2.1.285 | 2.1.285 |

Older versions are refused. Other admitted versions are marked `untested`;
version admission does not establish feature support. API-key presence checks
remain local and report no observed provider version until a provider is probed.
Codex's offline schema is checked before native conversation work, including
cleanup methods, dynamic tools and branch cutoff fields. Its initialization
reply and observed version are retained in private provider events.

All admitted Claude versions can log in, run, recover, repair, prune and branch.
No exact-version or executable-hash namespace audit is required. Login and
isolated repair rely on native account status. Before deleting a macOS Keychain
item, Hive checks its computed service and recorded account and targets only that
item's persistent identity. Missing or changed items refuse only that operation.
Every branch checks the actual copied rooted conversation chain while holding its
source. Tool authorization and visible output wait for verification and durable
binding; the first real input may begin inference during that check. A failed
branch does not authorize replay. No extra model verification turns are submitted.
Same-turn steering keeps its separate executable-bound canary check.

After changing a selected provider executable, use `hivectl daemon restart`.
Executable replacement invalidates cached capability evidence; an unchanged version
string does not preserve it.

These are the supported-release policy, not a promise that current development
state can be upgraded. Retain the old data and executable until a replacement has
been checked for your use.
