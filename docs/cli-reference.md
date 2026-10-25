# Hive CLI reference

This is the complete command reference: every command, option, configuration setting
and example. The [README](../README.md) gives the overview and a quick start. The
bundled [agent skill](../skills/hive/SKILL.md) explains how an AI agent should
operate Hive and links here for commands. Capitalized IDs and paths in examples
are placeholders.

## Contents

- [Global options and output](#global-options-and-output)
- [Command examples](#command-examples)
- [Configuration and capacity](#configuration-and-capacity)
- [Setup, diagnostics and services](#setup-diagnostics-and-services)
- [Daemon](#daemon)
- [State upgrades](#state-upgrades)
- [Authentication and model discovery](#authentication-and-model-discovery)
- [Spawn and branch agents](#spawn-and-branch-agents)
- [Inspect agents, messages, and output](#inspect-agents-messages-and-output)
- [Send, steer, redirect, and schedule](#send-steer-redirect-and-schedule)
- [Files and images](#files-and-images)
- [Wait for results and external MCP](#wait-for-results-and-external-mcp)
- [Change agent options or login](#change-agent-options-or-login)
- [Agent names](#agent-names)
- [Human questions and approvals](#human-questions-and-approvals)
- [Stop, resume, interrupt, complete, terminate](#stop-resume-interrupt-complete-terminate)
- [Batch operations](#batch-operations)
- [Collect replies with a join](#collect-replies-with-a-join)
- [Purge, garbage collection, and compaction](#purge-garbage-collection-and-compaction)
- [Hosted agent tools](#hosted-agent-tools)

## Global options and output

```text
hive [GLOBAL OPTIONS] COMMAND [COMMAND OPTIONS]
hivectl --help
hive COMMAND --help
hivectl --version
```

Global options can appear before or after a subcommand. An explicitly supplied
option must be used by that command, even when its value equals the default.
Unused options fail before reading payload files, opening state or contacting a
provider. Omitted defaults do not cause conflicts.

| Option | Commands accepting it |
| --- | --- |
| `--config` | `setup`, `service` and `daemon` subcommands, `config check`, `doctor`, `models`, `verify-steering`, `auth list/status/login/recover/repair`, `tui` |
| `--state-dir` | All commands except `version` and `config check` |
| `--codex-bin`, `--claude-bin` | `setup`, `doctor`, `models`, `auth status/login/recover/repair`, `daemon run/start/restart`, `service install`; `verify-steering` also accepts `--claude-bin` |
| `--probe-timeout` | `setup`, `doctor`, `auth status/login/recover/repair` |
| `--login-timeout` | `setup`, `auth login/repair` |

For `setup --uninstall`, explicit `--config` and `--state-dir` require `--purge`;
omitted config/state defaults remain allowed. Provider executable and timeout
overrides are rejected in either uninstall mode. Uninstall also rejects supplied
wizard options, including `=false` and values loaded from `--answers`. See the
[removal option rules](setup.md#hive-setup-flags).

For setup and service installation, an explicit `--service-path` requires
colon-separated absolute existing directories, without empty entries or control
characters. Invalid explicit entries are rejected instead of discarded.

Remote agent commands use the daemon's saved configuration and accept only
`--state-dir` from these globals. Daemon stop/status retain `--config` because it
also identifies a managed service. Provider selection within a supported command
still depends on the selected profile or setup choices.

| Option | Meaning and default |
| --- | --- |
| `--config PATH` | YAML for auth, model discovery, verification, and daemon startup. Default: `$XDG_CONFIG_HOME/hive/config.yaml`, otherwise `~/.config/hive/config.yaml`. Relative paths use the invoking directory. |
| `--state-dir PATH` | Absolute, private, local state root selecting the controller and its daemon. Default: `$XDG_STATE_HOME/hive`, otherwise `~/.local/state/hive`. |
| `--codex-bin NAME_OR_PATH` | Codex executable; default `codex`. A command name or absolute path is accepted; relative executable paths are rejected. |
| `--claude-bin NAME_OR_PATH` | Claude executable; default `claude`. Same path rules. |
| `--probe-timeout SECONDS` | Auth helper timeout, default 15; range 1–300. Does not change runtime turn deadlines. |
| `--login-timeout SECONDS` | Interactive login timeout, default 600; range 1–3600. |
| `-h`, `--help` | Help for the selected command. |
| `-V`, `--version` | Hive version at the root command. |

For example, these commands use a separate controller and explicit provider path:

```sh
hivectl --config /absolute/config.yaml --state-dir /absolute/private/state \
  --codex-bin /absolute/bin/codex --probe-timeout 30 --login-timeout 900 \
  auth login codex-main --browser
hivectl --config /absolute/config.yaml --state-dir /absolute/private/state \
  --codex-bin /absolute/bin/codex daemon start
hivectl --state-dir /absolute/private/state agents
```

Ordinary commands print one JSON response. Errors have
`error.code` and `error.message`. Help and `version` without `--json` are text;
interactive login first gives the terminal to the provider; `mcp` uses the MCP stream instead of ordinary JSON output.
Blocking sends print progress to stderr while keeping their final JSON on stdout.
`read --follow` prints newline-delimited event pages on stdout; it has no final
summary response. Input rejections and observation failures add one JSONL error
record on stdout and a human diagnostic on stderr. If stdout itself fails, the
viewer reports the failure on stderr without appending to the damaged stream.
Closing a follow viewer's output pipe ends observation successfully; it does not
stop the agent. MCP runtime failures use stderr only and leave stdout for JSON-RPC.

Exit codes:

| Code | Meaning |
| --- | --- |
| `0` | Successful command/report. Auth status can report `missing`, `profile_busy`, or `unavailable` and still exit 0; inspect its state. |
| `1` | An operational failure (configuration, state, transport, protocol or output), or an unsuccessful outcome such as failed login, a wait timeout or a blocking call without a reply. |
| `2` | Invalid caller input, whether detected locally or by the daemon. |

Malformed command-line arguments are rejected before configuration, state or
provider access, including for `mcp` and `read --follow`. They return a JSON
`error` on stdout with `code`, `message`, `field`, `expected` and `examples`, plus
a short diagnostic and the same examples on stderr; exit code is 2. Public numeric
and enum values may appear in `received`. Private input, paths and unknown arguments
are omitted. Explicit `--help` and `--version` print text and exit 0. If a parser or
output operation itself fails, the command exits 1. Ordinary command output is
flushed before success is reported. Local runtime errors retain JSON on stdout
and include a short public diagnostic on stderr; raw causes are not printed.

A managed daemon that records a permanent startup failure exits successfully to
prevent a service restart loop, even if writing its diagnostic fails. Inspect
`hivectl service status` and fix the recorded configuration issue before restarting.

Field-specific daemon input errors use the same CLI argument names and exit code. Optional
control fields must have the advertised type when supplied: omit them to use a
default. For example, a non-string send mode or spawn model is rejected rather than
silently selecting the default. Explicit null is not omission, except where an
operation documents a nullable field.

Pagination errors reported by the daemon for `agents` (including `--stalled`),
`messages`, `read` and `output` use the same structured input-error contract and
exit 2. Fields name the relevant CLI flag or positional argument; examples match
command help. Daemon errors never include `received`. A response's `request_id`,
when present, is retained for retries. Other errors after parsing retain the
command-specific output behavior described above. A failed observation of an
already accepted blocking call preserves its message and request IDs; it is not
reported as rejection of the original command.

`--body-file` and `--result-file` read nonempty UTF-8 text, at most 65,536 bytes.
`--instructions-file` has the same size cap but may be empty. Whitespace is retained.

The complete instruction snapshot, including shared context and its separators,
must fit within 262,144 UTF-8 bytes. An explicit instruction override that pushes
an otherwise valid snapshot over this cap reports `--instructions-file`; oversized
configured context remains a configuration/runtime failure.
`--summary-file` and inline evidence summaries are limited to 8,192 bytes and must
not be blank. Approval `--data-file` is limited to 60 KiB and accepts any JSON value;
its content must also satisfy the selected approval's form. Batch
`--operations-file` is limited to 512 KiB and must contain a JSON array.

The client prepares each selected file once before configuration/default-path,
service or daemon work. Missing, unreadable, malformed and oversized inputs return
a structured input error naming the flag, with examples and exit 2. The local human
diagnostic also includes an escaped, bounded `Path:` line; paths and file contents
are omitted from JSON and daemon/MCP errors. Native resource/I/O failures exit 1.

Regular files and symlinks to them are supported. Pipes, heredocs and process
substitution are also supported, with a 30-second total deadline to receive EOF;
partial data does not extend it. Hive must be the pipe's only reader. Shared file
descriptor flags are left unchanged. Directories, sockets and devices (including
interactive terminal input) are rejected. A file named `-` is a literal filename;
use `/dev/stdin` for piped input. For example:

```sh
printf 'Review the current diff.\n' | hivectl send AGENT_ID --body-file /dev/stdin
# Bash or Zsh:
hivectl send AGENT_ID --body-file <(printf 'Review the current diff.\n')
```

Paths to content files are resolved by the client. Attachment paths keep their
separate snapshot/retry rules; they are not opened during this preparation step.

For CLI mutations that offer `--request-id`, supply a stable ID (1–128 UTF-8 bytes, no control characters) when
retrying matters. Retry a lost acknowledgement with the **same ID and arguments**;
a different ID is a new operation. Conflicting reuse is rejected. Acceptance means
durable storage, not successful execution or provider delivery.

A retry returns the original saved acknowledgement, including for steering;
inspect message status to observe later delivery. If an upgrade changes the
response protocol, the retry returns `replay_unavailable` with the original
`request_id` instead of executing again or reshaping the old response. Keep that
ID and inspect the original operation before issuing any new request.

Remote commands use the daemon's configuration. With an explicit `--state-dir`,
they do not need a local configuration file or configuration-directory environment
variables. `auth groups` and `auth usage` read from the daemon. `auth generations`, `auth prune`
and `auth history-export` use the selected state directory directly and retain
its ownership and format checks. `auth list` reads configuration and saved profile
bindings without starting providers. Setup, doctor, service management, daemon
control, login/probe commands and model discovery still use configuration. Daemon
stop/status use its path to identify the managed service.

Agent IDs and message IDs are opaque, nonempty UTF-8 strings up to 65,536 bytes;
pass the returned value unchanged. `wait --join` accepts up to 65,531 bytes because
its read target includes the `join:` prefix. Profile, role and collector destination
names use 1–64 ASCII letters, digits, `_` or `-`. Invalid selections are rejected
before reading task files or contacting the controller; unknown valid names are
resolved by the command that consumes them.

For `spawn`, `set-options` and `set-auth`, rejected explicit selections identify
the field to correct: unknown role/login names, model or effort absent from the
selected catalog, conflicting provider, unsupported option or invalid type.
These are input errors (CLI exit 2). Model and effort must be supported together;
supply both when changing to a model that does not support the retained effort.
Invalid configured defaults, inherited selections and provider-discovery failures
remain operational errors.

## Command examples

These are also the examples shown by each command’s help and argument errors.
Replace sample IDs, profiles and file paths with your own values. `evidence` and
`collectors` require the Comb-enabled build.

| Command | Example |
| --- | --- |
| `hivectl` | `hivectl --help` |
| `hivectl setup` | `hivectl setup`<br>`hivectl setup --yes --answers answers.yaml --dry-run` |
| `hivectl service` | `hivectl service status` |
| `hivectl service install` | `hivectl service install` |
| `hivectl service start` | `hivectl service start` |
| `hivectl service stop` | `hivectl service stop` |
| `hivectl service status` | `hivectl service status` |
| `hivectl service uninstall` | `hivectl service uninstall` |
| `hivectl doctor` | `hivectl doctor` |
| `hivectl config` | `hivectl config check` |
| `hivectl config check` | `hivectl config check`<br>`hivectl config check --fix` |
| `hivectl version` | `hivectl version --json` |
| `hivectl state` | `hivectl state status` |
| `hivectl state status` | `hivectl --state-dir /private/hive-state state status` |
| `hivectl state upgrade` | `hivectl --state-dir /private/hive-state state upgrade --dry-run` |
| `hivectl logs` | `hivectl logs --tail 100`<br>`hivectl logs --level warn --follow` |
| `hivectl approvals` | `hivectl approvals --wait --timeout 60` |
| `hivectl approvals show` | `hivectl approvals show 00000000-0000-4000-8000-000000000001` |
| `hivectl respond` | `hivectl respond 00000000-0000-4000-8000-000000000001 --accept --request-id answer-1` |
| `hivectl mcp` | `hivectl mcp` |
| `hivectl auth` | `hivectl auth list` |
| `hivectl auth groups` | `hivectl auth groups` |
| `hivectl auth usage` | `hivectl auth usage`<br>`hivectl auth usage codex-main` |
| `hivectl auth list` | `hivectl auth list` |
| `hivectl auth status` | `hivectl auth status codex-main` |
| `hivectl auth login` | `hivectl auth login codex-main --browser` |
| `hivectl auth recover` | `hivectl auth recover codex-main` |
| `hivectl auth generations` | `hivectl auth generations codex-main` |
| `hivectl auth history-export` | `hivectl auth history-export --output /private/controller-history` |
| `hivectl auth prune` | `hivectl auth prune codex-main --generation 00000000-0000-4000-8000-000000000001` |
| `hivectl auth repair` | `hivectl auth repair codex-main --reauthenticate --browser` |
| `hivectl compact` | `hivectl compact --before 1700000000 --keep-per-agent 100` |
| `hivectl output` | `hivectl output 00000000-0000-4000-8000-000000000001 --offset 0 --limit 65536` |
| `hivectl models` | `hivectl models --auth codex-main` |
| `hivectl verify-steering` | `hivectl verify-steering --auth claude-main --model sonnet --effort low` |
| `hivectl daemon` | `hivectl daemon status` |
| `hivectl daemon run` | `hivectl --config examples/auth.yaml daemon run` |
| `hivectl daemon start` | `hivectl --config examples/auth.yaml daemon start` |
| `hivectl daemon restart` | `hivectl --config examples/auth.yaml daemon restart` |
| `hivectl daemon stop` | `hivectl daemon stop` |
| `hivectl daemon status` | `hivectl daemon status` |
| `hivectl spawn` | `hivectl spawn reviewer --cwd /absolute/repo --body-file review.md` |
| `hivectl agents` | `hivectl agents --limit 100` |
| `hivectl batch` | `hivectl batch --operations-file batch.json --join --request-id batch-1` |
| `hivectl status` | `hivectl status AGENT_ID` |
| `hivectl read` | `hivectl read AGENT_ID --after 0 --limit 100`<br>`hivectl read AGENT_ID --follow` |
| `hivectl live` | `hivectl live AGENT_ID`<br>`hivectl live AGENT_ID --follow` |
| `hivectl attach` | `hivectl attach AGENT_ID`<br>`hivectl attach AGENT_ID --read-only`<br>`hivectl attach AGENT_ID --exit-on-end --linger 10 --title` |
| `hivectl panes` | `hivectl panes`<br>`hivectl panes --root AGENT_ID --max-panes 8 --linger 10` |
| `hivectl tui` | `hivectl tui`<br>`hivectl tui --root AGENT_ID --max-views 4 --keep-ended --quiet` |
| `hivectl messages` | `hivectl messages AGENT_ID --limit 100` |
| `hivectl send` | `hivectl send AGENT_ID --body Review --request-id follow-up-1`<br>`hivectl send AGENT_ID --body-file follow-up.md --request-id follow-up-2` |
| `hivectl set-auth` | `hivectl set-auth AGENT_ID codex-backup --request-id login-change-1` |
| `hivectl set-name` | `hivectl set-name AGENT_ID reviewer --request-id rename-1`<br>`hivectl set-name AGENT_ID --clear --request-id rename-2` |
| `hivectl set-options` | `hivectl set-options AGENT_ID --option model=gpt-6-luna --option effort=low --request-id settings-1` |
| `hivectl options catalog` | `hivectl options catalog --provider codex --model gpt-6-luna` |
| `hivectl complete` | `hivectl complete AGENT_ID --body Done --request-id finished-1`<br>`hivectl complete AGENT_ID --result-file result.md --request-id finished-2` |
| `hivectl evidence` | `hivectl evidence correction --summary-file correction.md --request-id correction-1` |
| `hivectl collectors` | `hivectl collectors status` |
| `hivectl collectors status` | `hivectl collectors status` |
| `hivectl collectors discard` | `hivectl collectors discard project` |
| `hivectl collectors release-uncertain` | `hivectl collectors release-uncertain project ITEM_ID` |
| `hivectl collectors rebind` | `hivectl collectors rebind project 00000000-0000-4000-8000-000000000001` |
| `hivectl stop` | `hivectl stop AGENT_ID --request-id stop-1` |
| `hivectl terminate` | `hivectl terminate AGENT_ID --request-id terminate-1` |
| `hivectl purge` | `hivectl purge AGENT_ID --dry-run` |
| `hivectl gc` | `hivectl gc --older-than 30d --dry-run` |
| `hivectl resume` | `hivectl resume AGENT_ID --request-id resume-1` |
| `hivectl interrupt` | `hivectl interrupt AGENT_ID --request-id interrupt-1` |
| `hivectl wait` | `hivectl wait AGENT_ID --until completed --timeout 60`<br>`hivectl wait --join JOIN_ID --timeout 45` |
| `hive-daemon` | `hive-daemon --config examples/auth.yaml` |

## Configuration and capacity

Validate the selected configuration locally:

```sh
hivectl --config examples/auth.yaml config check
hivectl --config examples/auth.yaml config check --fix
```

`check` validates syntax, known keys, values and configuration relationships. It
does not open agent state, contact a daemon or provider, read API-key contents, or
verify that referenced instruction files and external services are available.
The JSON reports `valid`, `changed`, `applied`, `deprecations` and `backup`, and
`auth_groups` with each group's normalized `mode`, `members`, balanced `weight`s and
`paid_tail`. Its load fields are `null` with `load_available: false`, because only
a running daemon knows them; use `auth groups` for live counts. Invalid input
fails without rewriting it. Use `doctor` for runtime readiness.

`--fix` applies only registered safe key renames, then validates the complete
result. A conflicting replacement key is refused even when both values agree.
An actual rewrite creates a private backup, preserves a configuration symlink,
and refuses concurrent edits before publication. The rewritten YAML may normalize
formatting and comments; the backup retains the exact original. A no-op preserves
bytes and metadata and creates no backup. Fixes do not reload a running daemon;
restart it explicitly after inspecting an actual change.

There are currently no deprecated keys or registered renames. Unknown keys stay
invalid, including names from incompatible development builds. When deprecations
are present, configuration loads emit `config.deprecated` warnings through the
configured diagnostic sink (stderr for ordinary CLI commands). `config check`
lists the key patterns, replacements, introduction versions, removal major
versions and affected counts. `daemon status` exposes the loaded configuration's
`deprecations`, which can differ from a file edited after the daemon started.
See the [public compatibility policy](compatibility.md#public-interfaces-and-deprecation).

Provider model names accept 1–128 ASCII letters, digits and `-_.:@/[]`, with no
leading `-`. This includes aliases such as `sonnet[1m]` and provider-qualified names.
Effort accepts `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max` or `ultra`.
The provider's model catalogue must still list the requested model and effort.
These syntax rules apply to provider settings in YAML and to CLI overrides,
including setup's Codex/Claude options; setup's embedding model selector has its
own rules.

Save YAML as the default config file or pass its path with `--config`. This example
provides both subscriptions and optional roles:

```yaml
auth_profiles:
  codex-main: {provider: codex, method: subscription}
  claude-main: {provider: claude, method: subscription}
default_provider: codex
providers:
  codex: {auth: codex-main, model: gpt-6-luna, effort: low}
  claude: {auth: claude-main, model: sonnet, effort: low}
roles:
  implementor:
    provider: codex
    sandbox: workspace-write
    lifetime: independent
  reviewer:
    provider: codex
    sandbox: read-only
    lifetime: independent
```

Then `spawn implementor --cwd DIR --body-file task.md` uses that role, and
`spawn --provider claude --cwd DIR --body-file task.md` uses Claude defaults.
Start with [examples/auth.yaml](../examples/auth.yaml) and add roles as above.
A human spawn uses `independent`; `with_parent` requires an agent parent.

| YAML field | Purpose |
| --- | --- |
| `auth_profiles` | Named profiles: `provider: codex\|claude`, `method: subscription\|api_key`. API profiles require exactly one of `api_key_env` or `api_key_file`; subscription profiles must set neither. |
| `auth_groups` | Named [login groups](#login-groups) of one provider: `mode: ordered\|balanced`, subscription `members` (optional `weight` 1–1000 in balanced groups) and an optional ordered API-key `paid_tail`. |
| `default_provider` | Selects a provider when the request/role does not. |
| `providers` | Per-provider `auth`, `model`, and `effort` defaults. |
| `roles` | Optional presets: `provider`, `auth`, `model`, `effort`, `instructions`, `sandbox`, `lifetime`, `api_fallback`, `comb`, `mcp_servers`. |
| `comb` | Optional shared Comb binding: absolute `credential_file` and `reviewer` (default false). A role binding replaces the global binding. See [Comb setup](../comb/README.md#hive-agents). |
| `comb_collectors` | Opt-in named evidence destinations: absolute `credential_file`, selected `sources`, optional `enabled` (default true). Global only; maximum 16 retained names. See [shared learning evidence](#shared-learning-evidence). |
| `mcp_servers` | Named stdio servers with `command`, optional `args` list and `env` mapping. Role entries replace global entries by name. |
| `context` | List of shared instruction-file paths prepended to agent instructions. |
| `runtime` | Scheduling/residency limits below. |

Instruction paths in YAML are relative to the config file, and their contents
are saved when the agent is created. Explicit spawn settings precede role settings,
then provider defaults. The selected auth profile also determines the provider;
contradictions are errors. A sole configured provider/profile can be inferred;
ambiguous choices require defaults or explicit flags. Model must resolve; effort
ultimately defaults to `high` and is validated against the provider catalog.

`--instructions-file` replaces role instructions, while shared `context` still
applies. An empty file clears the role's portion. Branch inheritance has its own
rules described below. Configuration is loaded at daemon startup: restart the
daemon to load edits; existing agents retain their saved settings.

Additional tools are optional. For example:

```yaml
comb:
  credential_file: /private/comb-bridge.json
mcp_servers:
  project_docs:
    command: /absolute/path/to/docs-mcp
    args: [--stdio]
    env: {DOCS_INDEX: /absolute/path/to/docs}
```

Extra MCP servers use native provider stdio transport. Up to 16 servers may be
configured after role merging; names use letters, digits, `_` or `-`, and cannot
be `hive` or contain `__`. Each server allows 64 arguments and 64 environment
entries; individual values are capped at 4 KiB and the merged map at 64 KiB.
The saved settings apply at start/resume and are inherited by branches without a
new role. Public status shows server names, not command arguments or environment.
Comb credentials are read by the daemon; native provider configuration contains
only its Comb tool catalog. A Codex branch with an explicit role must retain the
source's enabled Comb/reviewer catalog. Changing that catalog requires a new
ordinary agent. See [Comb commands and examples](../comb/README.md).

Capacity example (these are explicit example limits, not shipping defaults):

```yaml
runtime:
  max_agents: 1000
  max_loaded: 32
  max_providers: 32
  max_process_variants: 8
  sessions_per_codex_process: 16
  max_starting: 8
  max_active_turns: 16
  idle_seconds: 30
  cohort_min_seconds: 5
  warm_providers: 0
  warm_idle_seconds: 60
  linger_seconds: 2
  live_stream: true
  allow_spawn_widening: false
```

| Setting | Default | Meaning |
| --- | ---: | --- |
| `max_agents` | 1000 | Nonterminal or still-draining agents; stopped terminal history does not consume slots. Maximum 1,000,000. |
| `max_loaded` | 256 | Resident conversations, distinct from total durable agents. |
| `max_providers` | 128 | Concurrent provider processes. |
| `max_process_variants` | 8 | Distinct process-setting combinations per provider/login; positive, at most 1,000,000. Desired and reserved settings both count until the agent ends. |
| `sessions_per_codex_process` | 16 | Conversations one Codex process hosts over its lifetime, up to 1,024. A finished conversation keeps its slot until the process retires, so capacity returns through process turnover (see `linger_seconds`). Claude remains one conversation per process. |
| `max_starting` | 8 | Concurrent agent initialization. |
| `max_active_turns` | 128 | Concurrent inference turns. |
| `idle_seconds` | 0 | Idle conversation unload delay; zero disables ordinary idle unloading. Capacity/control can still unload it. Maximum 86,400. |
| `cohort_min_seconds` | 5 | Minimum provider cohort residency before eviction; maximum 3,600. |
| `warm_providers` | 0 | Optional unused Codex provider spares; zero disables them. |
| `warm_idle_seconds` | 60 | Warm spare idle expiry, 1–3,600 seconds. |
| `linger_seconds` | 2 | Grace for reusing an empty multiplexed provider; zero disables it, maximum 3,600. |
| `live_stream` | true | Stream assistant text, command output and reasoning summaries to [live viewers](#watch-live-output). False keeps Codex deltas switched off at the provider. |
| `allow_spawn_widening` | false | Allow hosted agents to spawn children in wider sandbox modes, including role defaults and batch spawns. This lifts the caller's ceiling for branches too; every branch still retains its source's mode. |

Positive limits must satisfy `max_providers`, `max_starting`, and
`max_active_turns` ≤ `max_loaded` ≤ `max_agents`; warm providers cannot exceed
`max_providers`. Increasing durable capacity does not require loading every agent.
Provider memory and account limits still constrain real concurrency. Choose residency limits according to available memory and provider process costs.

## Setup, diagnostics and services

```sh
hivectl setup
hivectl setup --yes --answers answers.yaml --dry-run
hivectl doctor
hivectl version --json
hivectl service status
hivectl service stop
hivectl service start
hivectl service uninstall
```

`setup` detects/authenticates providers, saves provider model defaults, optionally
connects Comb and installs user login services. Every setup flag, answers-file
key and removal choice is in the [setup command guide](setup.md#hive-setup-flags).
The guide also covers Comb setup/model commands and the installer.

`setup --embedding-config FILE` requires a readable regular file; symlinks to
regular files are supported. Hive invokes Comb separately for preview and apply,
so keep the file unchanged throughout setup. For configuration from a
single-consumer pipe, invoke `comb setup` directly instead.


`doctor` checks build capabilities, provider versions/auth, daemon health/version,
services, Comb reachability/backfill and bundled/installed skill copies. It
prints a JSON report with repair commands and a stderr summary; error checks
produce an unsuccessful exit. `hivectl version` prints one text line with the package
version and build variant, for example `Hive 0.1.0 (comb)`. `hivectl version --json`
reports `product`, `product_version`, `commit` and `compiled_features` as JSON.
`commit` is the checkout HEAD captured during compilation, or `null` for an archive
without Git; it does not certify an unmodified checkout. Both forms
work without a home directory, configuration, state directory or daemon.

`service install` registers this configuration/state scope. Optional
`--environment-file FILE`, `--program PATH`, and `--service-path PATH` select a
private literal environment file, stable executable, and toolchain search path.
Use `service start` to run it; `stop` keeps registration, `uninstall` removes the
owned registration and preserves binaries/data. `status` reports manager,
readiness, exact paths and errors. Service details and examples are in
[the setup guide](setup.md#services).

Inspect state compatibility with `hivectl state upgrade --dry-run` before changing builds.

## Daemon

`hive-daemon [GLOBAL OPTIONS]` runs the controller in the foreground and accepts only
the global options above. Use `hive-daemon --help` for its dedicated help; do not pass
`daemon run` or another command to `hive-daemon`.

| Command | Behavior |
| --- | --- |
| `daemon start` | Start in the background, or return status when the running daemon has the same startup inputs. Conflicting inputs require an explicit stop/start. |
| `daemon run` | Run in the foreground. |
| `daemon status` | Show liveness/PID, frozen executable SHA-256, product/commit identity, supported control protocols and agent catalog, storage diagnostics, resident actors, profile leases, provider capabilities, and writer observation metadata. |
| `daemon restart` | Stop the current daemon, confirm ownership cleanup, then start the selected build and configuration. Saved agents and input remain; uncertain turns are never replayed. |
| `daemon stop` | Request orderly shutdown; the acknowledgement is not proof that every process has exited. |

```sh
hivectl --config examples/auth.yaml daemon start
hivectl daemon status
hivectl --config examples/auth.yaml daemon restart
hivectl daemon stop
# After shutdown has finished, run in the foreground:
hivectl --config examples/auth.yaml daemon run
# Equivalent foreground executable:
hive-daemon --config examples/auth.yaml
```

Only one daemon owns a state root. Closing the terminal does not stop a detached
daemon. Runtime files are under `STATE/runtime/`, including `runtime.sqlite`,
`control.sock`, and `daemon.log`. Provider executable choices are resolved at
startup. A missing executable affects that provider's work, not the other provider.

On startup, after ownership and state-format checks, the daemon removes abandoned
state preparations from the state root, `runtime/`, and `runtime/output/`. Cleanup
holds the publishers' locks, does not recurse, and refuses unsafe entries. It
leaves provider histories, credentials, attachments, and ordinary user exports
alone. Cleanup failures stop startup; correct the reported ownership or filesystem
problem before retrying.

Output basenames `.hive-tmp-state-` or `.comb-tmp-admin-` followed by exactly
six ASCII letters/digits or sixteen lowercase hexadecimal digits are reserved
for Hive state or Comb administration preparations.
Do not use them for history exports, setup configurations (including symlink
targets), Comb credentials, bridges or custom sockets. Input files and attachment
basenames may still use these names.

Repeating `daemon start` compares the canonical configuration path and exact file
bytes, plus the selected provider launch paths and the selected Hive executable
SHA-256. A different executable returns `daemon_binary_mismatch` with instructions
to run `daemon restart`. It never reloads or restarts a
running daemon. A mismatch names `--config`, `--codex-bin` or `--claude-bin`;
inspect `daemon status`, stop the daemon, wait for shutdown, then start with the
new inputs, or use explicit `daemon restart`. Restart interrupts ongoing work
through orderly shutdown and waits for the original ownership checks. If cleanup
cannot be confirmed, it refuses to start another process. A lost shutdown reply
is not retried automatically. Provider fingerprints compare launch paths, not executable contents.
Permission errors, timeouts and invalid replies stop startup instead of launching
another process. A failed readiness check after launch can leave the daemon
running: inspect its status and logs, then stop it before retrying.

For an installed user service, `daemon start` compares the loaded configuration
and retains the service's provider paths and environment. Explicit `--codex-bin`
or `--claude-bin` options are rejected even when equal to their defaults. To
change them, stop the service and update its registration with `service install`.
`daemon restart` uses that installed registration and its environment; it does
not replace them with terminal provider options. Managed status includes the
running binary and protocol identities in its `daemon` object (`null` when stopped).

Daemon diagnostics are written to `STATE/runtime/logs/hive-daemon.jsonl`, with up to
five files of 10 MiB each. The files are private to your user. Entries include
lifecycle events and sanitized error kinds; message bodies and credentials are
excluded. `runtime/daemon.log` contains early stdout/stderr from `daemon start`.
Each start appends a separator and preserves earlier output. Child output may
split lines; this raw file is not JSONL. After safe ownership admission, the process
checks it at startup and once per second. Above 10 MiB it atomically saves the last
1 MiB to a `.prev` sibling before truncating the same active inode. Writers keep
appending to that inode; it is never renamed. A failed snapshot preserves the
active bytes, and a competing starter cannot trim the current owner's output.
This is soft retention: it can overshoot between checks, while the owner is stuck,
or during failures before ownership admission. Concurrent writes during a trim
can be discarded. These files are troubleshooting output, not audit receipts.
Managed service stdout/stderr paths are reported by `hivectl service status`.
Raw stdout/stderr, including Rust panic text, does not
have the structured log's privacy filtering and is not automatically exported.

The default diagnostic level is `info`. Configure the level and retention in
the main configuration file:

```yaml
logging:
  level: info
  file_bytes: 10485760
  files: 5
```

`files` includes the current file and accepts 2–64. `file_bytes` accepts
12,288–1,073,741,824 bytes. Invalid limits are rejected before daemon startup.
`HIVE_LOG` overrides only the level: `off`, `error`, `warn`, `info`, `debug` or
`trace`. Standalone CLI diagnostics use the same privacy filtering and bounded
queue on stderr, at `info` unless `HIVE_LOG` overrides it; they create no log files.
An invalid override is reported and uses `info` for standalone commands, so you
can still inspect or stop a daemon. Starting a daemon rejects an invalid override
before creating state.

```sh
HIVE_LOG=debug hivectl --config examples/auth.yaml daemon run
hivectl --state-dir /path/to/state logs --tail 100
hivectl logs --level warn --follow
hivectl logs --event agent.completed --agent AGENT_ID
```

Dependency diagnostics remain capped at warnings and errors. Fatal failures still
emit at `off`. The bounded queue reports dropped records as `log.dropped`; a
shutdown that cannot confirm diagnostic writes reports failure without waiting
indefinitely for disk or stderr.

`logs` is a human viewer that works without a running daemon or a configuration
file. It prints JSONL records on stdout and viewing warnings on stderr. `--tail`
selects the latest 1–2,000 matching records (default 100), inspecting at most
8 MiB across retained files. `--level` selects that severity and more severe
records; `--event` and `--agent` match exact values. `--follow` continues across
rotation until Ctrl-C; stopping the viewer leaves the daemon and agents running.
It has no agent tool equivalent.

The viewer withholds incomplete trailing records and reports corrupt records,
retention gaps and the inspection limit. A slow viewer can miss pruned history;
diagnostics are not a lossless event stream. Unsafe files are refused. Viewing
missing logs does not create state. Lifecycle diagnostics appear only after the
transaction commits; per-turn details require `debug`.

At `debug`, daemon logs also include numeric timings for writer queue waits,
transactions, SQLite checkpoints and state-file synchronization. These help
investigate stalls, but add measurement overhead and share the bounded log queue
and retention limits. Use `info` for ordinary operation; timing records may be
dropped and are not a complete trace.

After an orderly restart, queued work wakes unpaused agents on their saved
conversations. Manual pauses persist. A crash can leave uncertain delivery or
cleanup fences; restoration of the daemon is not permission to replay that input.

## State upgrades

```sh
hivectl --state-dir /private/hive-state state status
hivectl --state-dir /private/hive-state state upgrade --dry-run
hivectl --state-dir /private/hive-state state upgrade
```

These local commands validate the state and report `format`, `dry_run`, `status`,
planned `steps`, completed `migrations` and retained `backups`. They require the
daemon, login/probe commands and native provider processes for that state to have
stopped. They never stop processes, initialize a missing root or accept `--config`.
`state status` performs the same inspection as `state upgrade --dry-run`.

There are no released predecessor formats yet: a valid current root reports
`status: "current"` and no steps. Incompatible development state is refused.
Backups are never removed automatically. If recovery reports a conflicting edit,
preserve the root, pending intent and backups; it will not overwrite that edit.
See [compatibility and upgrades](compatibility.md).

## Authentication and model discovery

Auth commands use the selected YAML, including commands inspecting existing
profiles. They do not run inference. `models` also performs discovery without an
inference turn. `verify-steering` deliberately runs one.
An unknown configured profile is an input error (exit 2), identifying `NAME` for
`auth status/login/recover/repair` or `--auth` for model discovery and steering
verification. State-only commands such as `generations` and `prune` do not require
that profile to remain in the configuration.

### List, inspect, and log in

```text
auth list
auth status PROFILE
auth login PROFILE [--browser] [--reauthenticate]
auth groups
auth usage [PROFILE]
auth generations PROFILE
models --auth PROFILE
```

```sh
hivectl --config examples/auth.yaml auth list
hivectl --config examples/auth.yaml auth status codex-main
hivectl --config examples/auth.yaml auth login codex-main
hivectl --config examples/auth.yaml auth login codex-main --browser
hivectl --config examples/auth.yaml auth login claude-main
hivectl --config examples/auth.yaml auth login codex-main --reauthenticate
hivectl auth generations codex-main
hivectl --config examples/auth.yaml models --auth claude-main
hivectl auth groups
hivectl auth usage
hivectl auth usage codex-main
```

- `list` lists configured profiles without starting providers or creating state.
- `status` checks credentials and reports the selected method, provider version,
  diagnostics, and availability when a daemon is reachable. `authenticated` does
  not guarantee an unexpired token or access to every model.
- Subscription `login` requires terminal stdin/stdout. Complete the native browser/device
  challenge yourself. Codex defaults to device login; `--browser` selects browser
  login. Claude uses its native subscription login. A compatible existing login
  returns `already_authenticated`.
- `--reauthenticate` explicitly requests another native login for the same method.
  Conflicting or unavailable pre-login status still blocks it; use recovery or
  isolated repair when appropriate.
- `groups` needs a running daemon and reports its configured fallback groups.
- `usage` needs a running daemon. It reports each login's newest quota usage, as
  the provider last reported it: `windows` (`five_hour`, `seven_day` and, for
  Claude, per-model weeks such as `seven_day_opus`; other Codex windows are
  `primary` or `secondary` with their `minutes`), each with `used_percent` and
  `resets_at` in Unix seconds, Codex `credits` (`has_credits`, `unlimited`,
  `balance`) and Claude `extra_usage` (`status` `allowed`, `allowed_warning` or
  `rejected`, and `in_use`). `observed_at` is when the daemon received it.
  Providers report usage during turns (Codex also when its process starts), so a
  login shows nothing until then and after a daemon restart. A repaired login
  reports each credential `generation` separately, newest first. A profile
  the daemon's configuration lacks is refused with `unknown_profile`. The control
  operation is `{"op":"usage"}`, with an optional `auth`; it is human-only and
  cannot be batched.
- `generations` lists active/retired login namespaces and their audit metadata.
- `models --auth` requires an individual registered profile, subscription or API-key. Use the returned
  model/effort pairs; catalog presence does not prove successful inference access.

At Codex startup and during model discovery, Hive retries an explicitly rejected
read-only `model/list` request at most twice across the entire paginated lookup,
waiting 250 ms and then 500 ms. This includes a native network timeout returned
as a request rejection. All pages and retries share a 30-second response budget;
an unanswered request keeps the full remaining budget and is not retried.
Transport writes and process cleanup retain their separate bounds.
Invalid protocol replies, process exit and I/O failures still fail;
initialization, authentication and agent turns are not retried by this policy.
Claude discovery is unchanged. A `provider.model_list_retry` diagnostic records
each retry without native response text.

### API-key profiles

API profiles run paid inference using an explicitly configured key. Set spending
limits at the provider; Hive does not enforce a dollar budget. Configure exactly
one source per profile:

```yaml
auth_profiles:
  codex-api:
    provider: codex
    method: api_key
    api_key_env: OPENAI_API_KEY
  claude-api:
    provider: claude
    method: api_key
    api_key_file: /absolute/private/anthropic-key
```

`api_key_env` is a variable name, never an interpolated key value. The daemon reads
its own environment at each provider acquisition; changing your terminal's
environment cannot update an existing daemon. Managed services need the declared
variables in their private `service install --environment-file FILE`; see
[service setup](setup.md#services). For foreground or manually started daemons,
provide the variable privately before starting the daemon.

`api_key_file` names an existing regular file owned by your user with no group or
other permissions (normally mode `0600`). It cannot be a symlink or hard link;
ancestors must satisfy Hive's trusted-directory checks. Relative paths resolve
against the configuration directory, and `..` is rejected. The key is nonempty
UTF-8, at most 16 KiB, with no whitespace or control characters. A file may end
with one LF or CRLF; environment values do not get newline trimming. Hive neither
creates the source file nor copies its key into its stored profile.

With that YAML saved as `api.yaml` and the sources already provisioned privately:

```sh
chmod 600 /absolute/private/anthropic-key
hivectl --config api.yaml auth login codex-api
hivectl --config api.yaml auth login claude-api
hivectl --config api.yaml daemon start
hivectl --config api.yaml auth status codex-api
hivectl --config api.yaml models --auth codex-api
# Replace MODEL and EFFORT with a pair returned by models:
hivectl --config api.yaml spawn --auth codex-api --model MODEL --effort EFFORT \
  --cwd . --body-file task.md
```

API `login` works without a terminal: it registers an isolated credential
generation/home and reports `outcome: registered`. Registration is idempotent;
retrying a missing source uses the same generation. A readable source reports
`present_unverified`; missing/empty sources report `missing`, and malformed or
unsafe sources include a diagnostic. None of these commands starts an inference
turn. Neither presence nor model discovery proves the key has remote access.

With a reachable daemon, `auth status` and API `login` use its source configuration
and environment and return `environment_source: daemon`. The CLI refuses a profile
that differs from the daemon's configuration. When the daemon is absent and no
controller owns the state, the CLI observes locally and labels it
`environment_source: cli`. A busy controller or an ambiguous connection failure
does not silently fall back to the terminal's environment.

Status never clears an invalid-key hold. After fixing a source, run `auth login
PROFILE` against the running daemon to request a fresh **unverified** attempt;
inspect `availability` and `warnings` to see whether restoration was accepted.
Offline API login only registers/checks the source. Resume manually paused agents
explicitly. Structured invalid-key failures and exhausted spending caps remain
unavailable; API rate limits are timed only when the provider supplies a usable
reset. There are no periodic paid API probe turns. Conversations and unsent input
survive these failures; uncertain submitted input is never replayed.

For file rotation, replace the file atomically while preserving its permissions.
New provider acquisitions use the replacement. For environment rotation, stop the
daemon, update its environment/service environment file and start it again.
Existing loaded conversations may keep their captured key, even for later turns,
until normal unload/relaunch. Rotation never interrupts an in-flight turn or
creates a new native conversation. If needed, stop and resume an agent to force
reacquisition after the source is updated. Changing to a model validated with the
replacement key also forces a safe relaunch before input reaches an older-key
process. Admission records non-secret `api_key_rotated` events; status includes
`execution_key` and `model_key` selection identities, not the secret itself.

Key values stay out of Hive arguments, databases, prompts, events and diagnostic
logs. Codex receives ephemeral API login through its native protocol; Claude
receives only the selected `ANTHROPIC_API_KEY` after environment scrubbing.
Provider policy or method conflicts refuse execution rather than selecting
ambient credentials. Profile ownership and recovery checks still apply.

`--browser` requires a Codex subscription profile; `--reauthenticate` requires a
subscription profile. Inapplicable explicit options are input errors before
native login starts. `verify-steering` requires a Claude subscription profile.

### Recover, repair, prune, and export

```text
auth recover PROFILE
auth repair PROFILE --reauthenticate [--browser] [--new-state-dir ABSOLUTE_PATH]
auth prune PROFILE --generation UUID
auth history-export --output PATH
```

```sh
hivectl auth recover codex-main
hivectl auth repair codex-main --reauthenticate --browser
hivectl auth status codex-main
hivectl auth generations codex-main

# Stop the daemon and wait for shutdown before pruning an unused retired generation:
hivectl daemon stop
hivectl auth prune codex-main --generation GENERATION_UUID

# For damaged controller ownership, authenticate in a new physical state root:
hivectl auth repair codex-main --reauthenticate \
  --new-state-dir /absolute/new/private-state
hivectl --state-dir /absolute/new/private-state auth history-export \
  --output /absolute/history.ndjson
```

`recover` reconciles saved process identities and credential-writer evidence.
It reports usable credentials separately from fenced conversations. It never
signals a saved PID merely because that number exists. A confirmed different
account requires isolated repair; transient account-probe failures remain retryable.

`repair` authenticates a fresh immutable generation and selects it for new agents.
It requires a subscription profile. In-place repair checks existing state format
before rejecting an inapplicable profile method or browser option.
It does not copy tokens, log out the old account, or change existing agents' pins.
`--new-state-dir` additionally creates a new controller without adopting damaged
old locks or databases; use that state root for subsequent commands.
Claude version probes use isolated temporary directories. Interrupted cleanup is
retried automatically when their state root is next used. Unknown or unresolved
evidence is retained; `metadata_probe_directory` names a retained repair wrapper,
and `warnings` reports cleanup failures separately from the probe result.

`prune` requires the daemon stopped and a retired generation with no remaining
agent references or unresolved ownership. It removes local credential files and
its scoped Claude Keychain item, not account-wide sessions.
On macOS, deletion looks up the computed service and the generation's recorded
Keychain account without reading the password. Missing or changed items are
refused. Deletion targets only the checked item's persistent identity; a saved
removal intent permits retry after deletion without touching a replacement.
Fresh login and isolated repair do not require this lookup.
It retains identity, lock and audit records. If interrupted, repeat the same
command: deletion resumes from its recorded intent. A completed prune is also
retryable and returns its original receipt. Missing or replaced ownership evidence
is refused; do not manually recreate credential directories or lock files.

`history-export` exports recorded retired-controller history from a verified copy,
including agents, messages, events, and independently checked ownership records.
It excludes credential contents and large output artifacts. It is not a general
export of the current live controller; a changing retired database is an error.

Claude login, repair and recovery use the selected executable's actual native
account status. Newer admitted versions report `untested`; no exact-version or
executable-hash audit is required. See [provider compatibility](compatibility.md).
Use the same supported `--claude-bin` for auth commands and daemon startup.
Keep executables unchanged during commands.
Recovery does not require a reboot.

## Spawn and branch agents

```text
hivectl spawn [ROLE] --cwd PATH (--body TEXT | --body-file FILE) [OPTIONS]
hivectl spawn [ROLE] --from SOURCE_ID (--body TEXT | --body-file FILE) [OPTIONS]
```

An explicit `--cwd` must resolve to an existing directory with a UTF-8 absolute
path. Hive checks it before reading task files or starting services. Symlinks are
accepted; the daemon still applies its canonical workspace and sandbox checks.
The CLI checks selected directories and task files on every invocation, so they
must remain accessible when retrying through the CLI. Accepted daemon requests
still replay before server-side directory checks. Branches may omit `--cwd` to
inherit the source directory.

A normal spawn creates an agent and its startup message. Acceptance is immediate;
the daemon starts the conversation when capacity is available. `ROLE` is optional
and names an entry in the daemon's `roles` configuration.

| Option | Meaning |
| --- | --- |
| `--body TEXT`, `--body-file FILE` | Initial task, supplied inline or as a UTF-8 file. Supply exactly one. |
| `--attach PATH` | Attach a file; repeat up to ten times. Relative paths use the invoking directory. See [files and images](#files-and-images). |
| `--cwd PATH` | Existing working directory; relative paths resolve against the invoking directory. Required for ordinary CLI spawns, inherited from the source for branches. |
| `--provider codex\|claude` | Provider override; otherwise selected by the role/defaults. |
| `--auth NAME` | Subscription/API-key profile or same-provider fallback group. |
| `--model MODEL`, `--effort E` | Shortcuts for the two typed catalog keys; validated together against the login catalog. |
| `--preset NAME` | Select a configured role; mutually exclusive with the positional role. |
| `--name NAME` | Display name for people; see [agent names](#agent-names). A branch starts unnamed. |
| `--option KEY=VALUE` | Repeatable typed catalog setting; JSON scalars are typed, other values are strings. Optional keys accept `null` to remove an inherited spawn setting. |
| `--native-config FILE` | Read up to 64 KiB of native Codex TOML or Claude JSON once; snapshot its bytes. Distinct from global `--config`. |
| `--instructions-file FILE` | Replace role instructions. An empty file clears role instructions; shared context still applies. |
| `--sandbox read-only\|workspace-write\|full-access` | Provider sandbox; default `workspace-write` unless the role specifies otherwise. Branches retain their source's mode. |
| `--lifetime independent` | Override the role's lifetime. Human CLI spawns have no parent; only `independent` is accepted. Hosted agent spawns can also select `with_parent`. |
| `--no-api-fallback` | Skip API members of a fallback group. Does not override an explicitly selected API profile. |
| `--request-id ID` | Stable identity for retries of this spawn. |
| `--from SOURCE_ID` | Branch an existing native conversation into a new agent. |

With the [configuration example](#configuration-and-capacity), start a configured
reviewer or override its settings:

```sh
hivectl spawn reviewer --cwd /absolute/path/to/repo \
  --body-file review-task.md --request-id reviewer-1
hivectl spawn implementor --cwd /absolute/path/to/repo \
  --model gpt-6-luna --effort low --instructions-file instructions.md \
  --lifetime independent --no-api-fallback \
  --body-file task.md --request-id implementation-1
```

`read-only` and `workspace-write` are provider policies, not a cross-provider OS
containment guarantee. Codex and Claude do not expose identical native tools.

`full-access` runs as your user without the provider's filesystem or network
sandbox. It can edit any path your user can write, run Git writes, install packages, read all login
credentials, alter Hive state, stop agents or the daemon, and push code. Hive
cannot protect its state or credentials from this mode. Codex receives
`danger-full-access` with approval policy `never`; Claude receives
`sandbox.enabled: false` and permits foreground Bash and file edits anywhere.
Claude's native subagent and cron tools remain disabled, and its foreground
shell ownership checks remain active. See the [security model](../README.md#security-model).

```yaml
roles:
  unrestricted:
    sandbox: full-access
```

```sh
hivectl spawn unrestricted --cwd /absolute/path/to/repo --body-file task.md
hivectl spawn --sandbox full-access --cwd /absolute/path/to/repo \
  --body "Make the change and commit it locally."
```

By default, authenticated agent spawns, including `hive_spawn` and batch spawns, reject a
child whose resolved mode is wider than the caller's:
`read-only` < `workspace-write` < `full-access`. Role defaults are checked too;
they are rejected rather than silently narrowed. For example, a read-only
agent must explicitly request `sandbox: read-only` when its chosen role defaults
to `workspace-write`. A full-access coordinator may spawn workers in any mode.
When widening is disabled, an attempted widening returns `binding_conflict`, as
does an attempt to change a branch's source mode.
Human CLI and TUI spawns have no such limit. This policy does not authenticate
human CLI access or create an OS isolation boundary.

The control socket treats connected clients as the human user and does not
attribute them to a hosted agent by process ID. A live check with Codex 0.163.0
in `workspace-write` on macOS was denied before connecting (`EPERM`); no request
reached the daemon. Linux socket access remains unverified. Do not assume the
same restriction on another platform or provider version.

To allow hosted agents to choose wider modes, set this in the daemon's YAML
configuration and restart the daemon to apply it:

```yaml
runtime:
  allow_spawn_widening: true
```

For example, this permits a read-only coordinator to spawn a workspace-write or
full-access worker. It affects explicit sandbox choices and role defaults in
ordinary, batch and branch spawns; it does not change an existing agent's sandbox.
Branches always retain their source's mode, and an explicit different mode on a
branch is rejected even with widening enabled.

`status`, `agents`, `tree` and spawn events carry the selected mode. Attach/TUI
headers and TUI tree rows mark full-access agents in red. `doctor` warns when
any role defaults to full-access or `runtime.allow_spawn_widening` is enabled.
Its `sandbox` check lists `full_access_roles` and the `allow_spawn_widening` flag,
including when no role defaults to full-access.

A read-only Claude agent's Bash runs only one read-only `git` command at a time:
- **Allowed subcommands:** `status`, `diff`, `log`, `show`, `blame`, `grep`,
  `ls-files`, `ls-tree`, `rev-parse`, `rev-list`, `merge-base`, `cat-file`,
  `describe`, `shortlog`, `diff-tree`, `for-each-ref` and `name-rev`. Also
  listing branches, `stash list`/`show` and `worktree list`.
- **What is refused:**
  - shell syntax: pipes, `;`, `&&`, redirection, substitution and backslashes;
  - options before the subcommand, such as `-C` or `-c`;
  - options that write a file or run another program: `--output`,
    `--ext-diff`, `--textconv`, `--filters`, signature checks and
    `grep -O`.
- **How it runs:** Hive replaces the command with a quoted form, adding
  `--no-pager`, optional index locks off and external diff and conversion
  programs off. Programs a repository configures, such as filters, can still
  run, but only inside the read-only sandbox.

### Branch in a new conversation

```sh
hivectl spawn --from SOURCE_ID --body-file alternative.md \
  --request-id alternative-1
hivectl spawn reviewer --from SOURCE_ID \
  --cwd /absolute/path/to/separate-worktree --body-file review-task.md \
  --request-id branch-review-1
```

The branch gets a new agent ID, mailbox, and native conversation. If the source
is running, the cut is **after its current turn finishes**. Otherwise it uses the
latest confirmed conversation boundary. The source's later queued input waits
until that cut has been materialized, then continues independently. A branch
request returns before this work finishes; inspect `status` for progress or failure.
Uncertain source input cannot be included speculatively.

Direct control and hosted-tool spawns require an absolute existing `cwd` unless a
branch source or calling parent supplies one. An explicit invalid path is rejected
instead of falling back. Directory symlinks are resolved before launch; the saved
path must also be valid UTF-8. The CLI accepts relative `--cwd` paths and resolves
them against its invoking directory.

Without an explicit role or overrides, the branch inherits the source's model,
effort, API-fallback preference, instructions, and working directory.
An explicit role supplies its settings; explicit flags take precedence.
The sandbox always stays equal to the source's mode, including when a different
role is chosen. An explicit different `--sandbox` is rejected. Agent callers
also cannot branch a source whose mode is wider than their own unless
`runtime.allow_spawn_widening` is enabled.
Lifetime is chosen separately: explicit `--lifetime`, then an explicitly selected
role's lifetime, otherwise `independent`. It is not inherited from the source.
The branch must initially use the **same provider,
login, and credential generation** as the source. Changing the model still has to
pass catalog validation. Codex branching requires advertised native cutoff support.
Claude clones the latest settled boundary while Hive holds the source; rewinding
to an earlier turn is not supported. Every branch checks the copied native
conversation chain against that boundary and its first submitted input. There is
no separate verification command or extra model turn. The real task may start
inference while history is being written, but Hive withholds tool authorization
and output until the copy verifies and the branch binding is saved. A failed
check refuses that branch; submitted input is never automatically replayed.

The check allows 15 seconds for native history and binding, reads up to 64 MiB
and 100,000 dialogue records per conversation, and holds at most 1,024 native
frames or 8 MiB. Unsupported or oversized native history refuses that branch.

The parent is the agent that called spawn, if any; `branched_from_agent_id` records the
source separately. Branching does not clone children, pending approvals, or files.
The agents share the working directory unless you choose another one; create a
worktree yourself when you need separate files.

A pending branch has a five-minute materialization deadline starting after the
source boundary is ready. Stopping it before its native conversation is bound
permanently cancels that branch request: `resume` will not turn it into a fresh
conversation.

## Inspect agents, messages, and output

```sh
hivectl agents --limit 100
hivectl agents --after 200 --limit 100
hivectl agents --parent PARENT_ID
hivectl agents --auth codex-main
hivectl status AGENT_ID
hivectl status AGENT_ID --instructions
hivectl read AGENT_ID --after 0 --limit 100
hivectl read AGENT_ID --follow
hivectl messages AGENT_ID --limit 100
hivectl messages AGENT_ID --before MESSAGE_CURSOR --limit 100
hivectl output 00000000-0000-4000-8000-000000000001 --offset 0 --limit 65536
```

| Command | What it returns and how to continue |
| --- | --- |
| `agents` | Compact summaries in ascending order, with `name` for named agents; pass `next_after` to `--after`. Default/max 200 rows. Optional `--parent` and `--auth` filters. |
| `status ID` | Process state, task outcome, desired/applied options and their sources, login, lifetime, branch state, pending approval/auth wait, and recovery/shutdown state. `--instructions` includes saved instructions. |
| `read ID` | Events after an exclusive numeric cursor. Range 1–200, default 100; continue with the returned `cursor`. Compacted history is identified by its pruning boundary. |
| `read ID --follow` | Human live view: drain retained event pages, then check for new events every 500 ms while idle. Each flushed JSON line is a page with a resume cursor. Ctrl-C ends observation. |
| `messages ID` | Incoming and outgoing messages, newest first. Range 1–200, default 100; pass `next_before` to `--before` until the page is empty. |
| `output ID` | A UTF-8 page from a retained large-output artifact, with byte `--offset` and `--limit` (range 4–65,536; default 65,536). Follow `next_offset` until `eof`. The artifact contains a JSON payload, not necessarily plain assistant text. |

`--parent` accepts a known agent ID, including a retained purged parent whose
children survived. `--auth` matches the current bound profile or assigned fallback
group; a queued login switch is not yet a match. A removed profile/group name still
works while an agent retains that binding. Unknown selectors are errors; a known
selector with no matching agents returns an empty page.

An unloaded or idle process is not a completed task. Inspect the task outcome as
well as process state. Message acceptance also differs from provider delivery:
messages can be queued, scheduled, submitted, finished, interrupted, undeliverable,
or uncertain.

Large events include an output descriptor and `read_target`. Agents can read that
artifact through `hive_read`; the CLI equivalent is:

```sh
hivectl read artifact:OUTPUT_ID --after 0 --limit 65536
```

Here `--after` is a byte offset, not an event cursor. Continue from `next_offset`.
The byte limit defaults to 65,536 and must be 4–65,536. Four bytes allow one complete
UTF-8 character. Row limits for `agents`, `messages` and ordinary `read` must be
1–200. Out-of-range limits are rejected; they are never silently reduced.
`read join:JOIN_ID` returns a whole snapshot and rejects `--after` and `--limit`,
even if their values are zero or the usual defaults.

### Watch one agent live

```sh
# Watch retained history and subsequent activity until Ctrl-C.
hivectl read AGENT_ID --follow

# Resume after the cursor from the last complete page you saved.
hivectl read AGENT_ID --follow --after 180 --limit 100

# Pretty-print individual events and retain explicit compacted-history notices.
hivectl read AGENT_ID --follow | jq --unbuffered \
  '(if .history_expired then {history_expired, pruned_through, cursor} else empty end), .events[]'
```

Follow is for humans watching a task or review exchange. Hosted agents should use
asynchronous messages and bounded `hive_read` calls; there is no follow tool.
It shows the same durable messages, activity and turn outcomes as ordinary `read`,
not a token-by-token provider stream. Large payloads remain artifact descriptors;
use `output` separately. Artifact targets cannot be followed.

Output is **JSONL pages**, not one JSON document or one event per line. Each page
has `events`, `cursor`, `pruned_through` and `history_expired`. `--after` defaults to
zero; `--limit` is a per-page count from 1 to 200 (default 100), not the total
number to print. Empty unchanged pages are silent. A pruning notice is printed
even when its event list is empty, then observation continues from its cursor.
Event sequence gaps alone are normal because sequences are shared across agents.

Follow stays open after turns and tasks end, including stop, failure and expiry,
so later events remain visible. Ctrl-C uses normal Unix interrupt behavior
(typically shell status 130) and never changes the agent. A slow output consumer
holds one bounded page in the CLI, not a provider turn or database transaction.
Closing its output pipe is treated as success when the next write detects it.

Operational daemon rejection, including an agent being purged, exits 1; input,
connection/protocol and other command errors exit 2. A daemon input rejection writes one final JSONL
error record after any pages already printed, plus a human diagnostic and examples
on stderr. Operational errors go only to stderr. Follow does not start or
reconnect the daemon automatically. Resume explicitly using the last **complete,
newline-terminated page's** cursor; discard a partial trailing line left by an
interruption. Existing socket limits apply per I/O, not as an overall follow
deadline.

### Watch live output

```sh
# One snapshot: status, newest events, counts and a live cursor.
hivectl live AGENT_ID

# The snapshot, then streamed output as JSONL until Ctrl-C.
hivectl live AGENT_ID --follow
```

Live output shows an agent's turn as it happens: assistant text, command output,
reasoning summaries and tool starts and ends, between `turn_started` and
`turn_completed`. Durable `read` history stays authoritative. Live items are never
stored; they exist only while someone watches.

The first line is the attach snapshot: `agent` (as `status` returns it), `events`
(the newest page, as `read` returns it, whose `cursor` resumes `read`), `queued`,
`mail`, `children` and `live`. Each later line is a page with `items`, `cursor`,
`resync` and `durable_changed`. Item kinds are `turn_started`, `text`,
`reasoning`, `tool_started`, `tool_output`, `tool_completed` and
`turn_completed`. Consecutive deltas of one item arrive merged within a page.

- `durable_changed: true` means new durable history exists. Call `read` with the
  snapshot's event cursor; live polling never replaces it.
- An assistant message's live `item_id` matches the `item_id` of its durable
  `assistant_message` event, so a viewer can replace streamed text with the
  stored message. Claude keys text by its native message, which can hold more
  than one text block; its aggregated message is the complete text.
- `resync: true` means some items were lost: the viewer fell behind the bounded
  buffer, the daemon restarted, or the watch expired. Reload `read` history and
  continue from the page's `cursor`. A missing cursor means the watch ended; `hive
  live --follow` then takes a new snapshot automatically.

A watch lasts 60 seconds after its last request. At most 256 agents can be
watched at once, and at most 32 long-polls can wait at once. Over either limit
the daemon answers `watch_capacity`; `hivectl live --follow` retries after one
second. A slow terminal never holds a provider turn or the database writer. For
an unwatched agent, streamed output is read and parsed, then dropped without
further work. Set `runtime.live_stream: false` to stop Codex from sending it.
Live output is human-only; agents cannot call it.

The control operations behind this command are `attach`, `live` (with the cursor,
`wait_ms` 0–25,000 and `limit` 1–512) and `tree`.

`attach` takes `limit` (1–200 rows) and at most one of two durable cursors:
- `after`: the page after that event sequence, as `read` returns it.
- `before`: the retained events before that sequence, oldest first.
  - Its `older` field continues backwards, and is `null` when no older retained
    events remain.
  - `history_expired` is true when older history was compacted away.
  - Pages stop early at their byte budget, keeping the newest rows.

Every reply also carries the agent snapshot, the counts and the live cursor.
When a child agent is spawned, its parent's viewers also see `durable_changed`.

`tree` returns compact rows for
an agent's subtree, or the live root agents without `root`. Each row holds:

- identity: `id`, `parent_id`, `role`;
- state: `state`, `task_outcome`, `waiting_reason`;
- login: `auth`, `auth_group`, `auth_decision`;
- settings: `model`, `effort`;
- `ended_at`;
- counts: `children`, `queued`, `mail`;
- the pending `approval_id`.

A reply holds up to 1,000 rows, with `truncated`. With `after` set to a returned
`version`, `tree` waits until agents change; replies come at most four times a
second. They are intended for terminal viewers. On daemon shutdown, waiting
requests end with `daemon_stopping`.

### Open a chat view

[Agents in your terminal](terminal.md) is a guide to `hivectl attach`, `hivectl panes`
and `hivectl tui`, with every key and the supported terminals.

```sh
# History, streamed output and an input line for one agent.
hivectl attach AGENT_ID

# Watch without writing; leave once the agent has ended.
hivectl attach AGENT_ID --read-only
hivectl attach AGENT_ID --exit-on-end
# As hivectl panes opens views: a 10 s linger and a findable terminal title.
hivectl attach AGENT_ID --exit-on-end --linger 10 --title
```

With `--exit-on-end`, the view leaves once the agent has ended and its final state
has been shown for `--linger` seconds (0–60, default 3). `--title`, also only with
`--exit-on-end`, titles the terminal `hive:<agent ID>` so `hivectl panes` can find
the view in tmux or Ghostty. It uses the resolved agent ID, updates after
`/attach` switches agents, and clears the title when the view exits.

`hivectl attach` shows one agent as a chat in the current terminal:
- a header with state, model and effort, login, parent, children and queue;
- the transcript: messages with their sender, assistant Markdown, tool calls
  with their last output lines, turn markers, steering and login notices, and
  questions;
- the active turn streaming below the stored history.

It needs a terminal for stdin and stdout; scripts use `hivectl live`. Closing the
view never stops the agent.

- **Agent not found.** If the agent does not exist, or the daemon refuses to
  attach, the view exits with that error, like any other command. A refused
  `/attach` returns to the agent that was open before.
- **Daemon unreachable.** The view waits and retries. It never starts the daemon
  itself.
- **Daemon restarts.** The header shows "disconnected", and the view reconnects
  without losing or repeating entries.
- **Daemon full.** When too many viewers are waiting, the view shows "waiting for
  a viewer slot" and keeps retrying.
- **Terminal closed.** Closing the window, the multiplexer pane or the SSH
  connection ends the view (and `hivectl panes` or `hivectl tui`) within a second;
  agents keep running.

| Key | Effect |
| --- | --- |
| Enter | Send. While a turn runs it steers when the agent supports steering, otherwise it queues |
| Alt-Enter | Always queue for the next turn |
| Ctrl-R | Redirect: interrupt, then deliver this text first |
| Esc Esc | Interrupt the running turn |
| Ctrl-J | New line in the input |
| Tab | Move focus between the input, a waiting question and the transcript |
| PgUp, PgDn | Scroll; scrolling past the top loads older history |
| Ctrl-C | Leave; asks again while a write is still in flight |
| Click, wheel | Focus the input, question or transcript (its Accept, Decline and Cancel buttons answer it); scroll the transcript |

The mouse is on by default; hold Shift to select text with the terminal (Option in
iTerm2), or pass `--no-mouse` to leave the mouse to the terminal.

Commands typed into the input:
- `/model NAME`, `/effort LEVEL` and `/options KEY=VALUE ...` change options.
- `/auth NAME` changes the login.
- `/stop`, `/resume`, `/interrupt` and `/terminate` control the agent.
- `/complete TEXT` marks the task complete with that result.
- `/branch TEXT` starts a branch of this conversation.
- `/spawn ROLE TEXT` starts a new root agent and prints `/attach ID`.
- `/attach ID` switches the view; `/quit` leaves.
- `/accept`, `/decline` and `/cancel` answer a waiting question.

A waiting question appears as a card, with clickable Accept, Decline and Cancel
buttons. On the focused card, `a` accepts, `d` declines and `c` cancels a simple
approval. A question with a form shows its
fields: arrow keys choose values and Enter accepts once the answer passes the
question's schema.

Each action is sent once with its own request ID. A retry after a lost reply
reuses that ID, so the daemon answers instead of acting twice. Diagnostics wait
until the view closes, then appear on stderr.

### Agent panes in zellij or tmux

```sh
# Inside zellij or tmux: one pane per live agent, opened and closed with them.
hivectl panes

# Only one agent and its descendants; at most 8 panes; panes linger 10 s.
hivectl panes --root AGENT_ID --max-panes 8 --linger 10
```

`hivectl panes` keeps one `hivectl attach` pane open for each live agent in scope, in
the multiplexer or terminal it runs in. Without `--backend zellij|tmux|ghostty`,
it chooses:
1. zellij or tmux, whichever runs this terminal (when one runs inside the other,
   the one that owns this terminal);
2. otherwise Ghostty 1.3 or newer, on macOS;
3. otherwise it exits with `pane_backend_unavailable`; use `hivectl tui` there.

A pane leaves on its own once its agent has ended and the linger has passed.
Leaving the controller (`q` or Ctrl-C) never closes panes or stops agents, and a
restarted controller adopts the panes already open.

If you close a pane yourself, it stays closed until you press `o` in the
controller. A pane that fails to start is retried after 5 s, 30 s and 2 min, then
listed as failed.

Each open view holds one of the daemon's 32 waiting requests. `--max-panes` (1–32,
default 16) leaves room for other viewers; agents beyond it are listed as "not
opened" and get a pane when one closes. If the daemon restarts, the controller
shows "disconnected" and carries on when it returns; when the daemon has no room
for another viewer, it shows "waiting for a viewer slot" and retries.

`--no-mouse` opens every pane's view without mouse support (each `hivectl attach`
gets `--no-mouse`), leaving clicks and the wheel to the terminal.

One controller runs per zellij session, tmux session or (for Ghostty) state
directory; a second one exits with `pane_controller_running`. A missing `zellij`
or `tmux` executable exits with `pane_backend_unavailable`.

#### zellij

- Each root agent gets its own tab, named after its role and short ID.
- Descendants are stacked in the tab of their nearest ancestor.
- Past six panes in a tab, a parent's further children share a `children` tab.

#### tmux

tmux 3.0 or newer.
- **Layout.** Each root agent gets a window in the controller's session, named
  after its role and short ID. Descendants split that window, tiled up to four
  agents; further children share a `children` window.
- **Focus.** New windows and panes never take focus from where you are.
- **Identity.** Views use `--title` and publish the agent currently shown in the
  pane title, including after `/attach` switches agents. Before a valid title
  arrives, the controller uses the start command, which also lets it adopt
  manually opened views without `--title`. The start command remains ownership
  evidence for cleanup.
- **Dead panes.** Agent windows have `remain-on-exit` off. If a hook or a pane
  setting keeps an ended view open anyway, the controller closes that dead pane,
  but only one it opened itself (with this state directory and `--exit-on-end`).

#### Ghostty on macOS

In a Ghostty 1.3 or newer terminal on macOS, outside zellij and tmux, `hivectl panes`
drives Ghostty itself.

- **Layout.** Each root agent gets a tab in the controller's window.
  Descendants are splits of that tab, up to a 2×2 grid of four agents; further
  children share a `children` tab.
- **Focus.** New tabs and splits never take keyboard focus from where you are
  typing.
- **Identity.** Views title their terminal `hive:<agent ID>`
  (`hivectl attach --title`), and a restarted controller adopts them by that title.
  The controller finds its own window the same way, by a unique title of its
  own. If Ghostty's `title` option is set, it overrides those titles, and
  `hivectl panes` refuses to start.
- **Automation permission.** The first run asks macOS for Automation
  permission. If it is denied, `hivectl panes` exits with
  `pane_backend_unavailable`; allow it in System Settings → Privacy & Security
  → Automation, or use zellij or tmux.
- **One controller.** One Ghostty controller runs per state directory.
- **Linux.** Ghostty has no scripting interface for tabs and splits on Linux;
  run `hivectl panes` inside zellij or tmux there.

### All agents in one terminal

```sh
# Every live agent: a tree sidebar, and tabs and splits of chat views.
hivectl tui

# One agent and its descendants; at most 4 views; ended agents stay listed.
hivectl tui --root AGENT_ID --max-views 4 --keep-ended --quiet
```

`hivectl tui` shows the agent tree in a sidebar and opens `hivectl attach` views in
tabs, at most two per tab, side by side or stacked. It works in any terminal,
including inside zellij and tmux, and never starts the daemon. Leaving it
(Ctrl-C, or the prefix and `q`) never stops agents; it asks again while a write
is still in flight.

- **Sidebar.** Agents are grouped under their parent, each with a badge: `!`
  needs you, `●` working, `◔` waiting for quota, `○` idle, and with
  `--keep-ended`, `✓` done, `✗` failed, or `–` cancelled or ended.
  - Left collapses a parent, which then shows how many agents it hides and
    their strongest badge (needs you, then working, then quota); Right expands
    it.
  - Clicking an agent opens its view. Typing filters by role, short ID or state;
    Enter opens the agent's view, or focuses the one it has.
- **Views.** The active view takes all of `hivectl attach`'s keys and commands;
  `/quit` closes the view and `/attach ID` opens or focuses another agent. A
  view closes 3 s after its agent ends, unless `--keep-ended` is set.
- **Mouse.** Clicks open agents, switch tabs, activate views and work the
  questions list and spawn dialog; the wheel scrolls the view under it. Hold
  Shift to select text (Option in iTerm2). `--no-mouse` leaves the mouse to the
  terminal; inside tmux, clicks arrive only after `set -g mouse on`.
- **Attention.** The prefix and `a` list every agent waiting for an answer, in
  the order this `hivectl tui` first saw them; the order restarts with it.
  - Enter opens the agent with its question focused.
  - A new question rings the terminal bell and shows the notification "Hive: N
    agents need you"; `--quiet` turns both off.
  - Ghostty, iTerm2 and WezTerm show the notification; Terminal.app does not.
    zellij and tmux do not pass it on, and whether they pass the bell depends on
    their settings.
- **Spawn.** The prefix and `c` open a dialog for the role, the login or login
  group, the model, the effort, the working directory (by default the current
  one) and the task.
  - Empty fields use the role's or the login's defaults.
  - The daemon's checks show at the field they concern, and the new agent opens
    in a new tab.
  - A dialog spawn has no parent: under `--root` it starts outside the TUI's
    scope, so reopen it later with `hivectl attach ID`.

Other keys start with a prefix, Ctrl-A by default, followed by one key. A button
bar on the last line (Sidebar, Split, Close, Questions, Spawn, Quit) does the
same with a click:

| After the prefix | Effect |
| --- | --- |
| `s` | Focus the sidebar; Esc returns to the view |
| `t` | Open the agent picked next in the sidebar in a new tab |
| `v` / `h` | Split the current tab left/right or top/bottom with the agent picked next |
| `o` | Switch to the other view of a split |
| `w` | Close the current view; the agent keeps running (asks again while a write is in flight) |
| `1`–`9`, `n`, `p` | Go to tab N, or the next or previous tab |
| `a` | Attention queue |
| `c` | Spawn dialog |
| `q` | Quit |
| The prefix again | Send the prefix key itself to the view |

An unbound key after the prefix is dropped, and a prefix expires after 1.5 s.
Neither zellij nor tmux binds Ctrl-A by default. To change the prefix or leave
the mouse to the terminal, write
`tui.toml` beside the configuration file (by default `~/.config/hive/tui.toml`,
or beside `--config`):

```toml
prefix = "ctrl-b"   # the prefix key (default "ctrl-a")
mouse = false       # leave the mouse to the terminal (default true)
```

A prefix is `ctrl-` followed by a letter or one of `]`, `\`, `^` and `_`. Ctrl-C,
H, I, J, M and R are refused, because views use them or terminals cannot tell
them from Tab, Enter or Backspace. A missing file means the default; an unreadable
one exits with `config_unavailable` and an invalid one with `config_invalid`.

Each open view and the sidebar hold one of the daemon's 32 waiting requests,
shared with `hivectl panes` and `hivectl live --follow`. `--max-views` (1–12, default
6) caps the views. A closed view's request can stay open for up to 25 s, so it
counts for that long. At the cap, opening another agent replaces the least
recently used view without a write in flight, or asks you to retry shortly. When
the daemon is full, views and the sidebar show "waiting for a viewer slot" and
retry.

### Find quiet groups

```sh
hivectl agents --stalled 15m --limit 100
# Copy next_cursor from the previous result as JSON:
hivectl agents --stalled 15m --stalled-after "$STALLED_CURSOR"
```

This reports whole live delegation groups that have had no activity for the
requested duration. Queued or scheduled work, active/submitting turns, pending
human questions, and auth waits prevent a group from being reported as stalled.
It is an observation, not an automatic nudge or termination policy.

`--stalled` accepts a positive integer with `s`, `m`, `h`, `d`, or `w`. It cannot
be combined with ordinary `--after`, `--parent`, or `--auth`. Stalled pages default
to 100 groups, max 200; pass the JSON `next_cursor` to `--stalled-after` until null.

## Send, steer, redirect, and schedule

```text
hivectl send TARGET (--body TEXT | --body-file FILE) [--mode next-turn|steer|redirect]
              [--attach PATH]... [--request-id ID]
              [--delay DURATION | --wait [--timeout SECONDS]]
```

The default `next-turn` mode durably queues one message for one future turn. A busy
agent handles it after current work. A manually stopped agent keeps it queued until
`resume`. Send returns a `message_id`; it does not normally wait for an answer.
Choose exactly one of `--body TEXT` or `--body-file FILE`. Both preserve exact
text, including whitespace, and accept 1–65,536 UTF-8 bytes. `--body` takes its
next argument literally, even when it begins with `-` (for example a Markdown
bullet or `---` heading). Quote text containing spaces/newlines; a trailing
`--body` without a value is an error. The same rule applies to spawn and complete.
For example:

```sh
hivectl send AGENT_ID --body "Review the latest diff." --request-id review-1
```

```sh
hivectl send AGENT_ID --body-file follow-up.md --request-id follow-up-1
hivectl send AGENT_ID --mode steer --body-file guidance.md --request-id steer-1
hivectl send AGENT_ID --mode redirect --body-file new-direction.md \
  --request-id redirect-1
hivectl send AGENT_ID --delay 10m --body-file reminder.md --request-id reminder-1
```

| Mode | Effect |
| --- | --- |
| `next-turn` | Queue ordinary input. It gets its own turn. |
| `steer` | Guide the current active turn at a supported provider boundary, without interrupting it or creating another turn. Requires `steering_supported`. |
| `redirect` | Interrupt the active turn and deliver guidance next, on the same conversation, before ordinary queued messages. |

**Steering** is bound to the execution and turn where it was accepted. A provider
receipt confirms delivery, not obedience. If the turn ends before delivery, the
message becomes `stale_turn`; it never spills into another turn. Multiple steers
are delivered in order. Unsupported steering and missing active turns are rejected;
Hive never silently converts a steer into a redirect.

Codex steering capability is discovered from its protocol. Claude steering uses
`PostToolUse` hook context and remains disabled until a native canary proves that
the selected binary/model/effort applies it. This command performs a small real
inference and must run while the daemon is stopped:

```sh
hivectl daemon stop
hivectl --config examples/auth.yaml verify-steering \
  --auth claude-main --model sonnet --effort low
hivectl --config examples/auth.yaml daemon start
hivectl status CLAUDE_AGENT_ID
```

Use the same `--claude-bin` when verifying and running. Verification is scoped to
the executable and resolved model/effort; updates can invalidate it. Wrapper
executables are not supported for this verification. A Claude turn with no later
tool boundary cannot receive held steering.

**Redirect** is one durable, deduplicated request. Retrying the same request ID
does not interrupt twice. If the turn finishes before interruption, the guidance
still becomes future input and the finished turn keeps its outcome. An interrupted
turn keeps its `interrupted` outcome. Startup input stays first; redirects keep
their own order before ordinary queued input. Redirect does not resume a manually
stopped agent. Uncertain deliveries are never replayed.

**Delayed input** accepts `--delay` from `1s` through `7d`, using the same duration
units as stalled detection. It is only available for `next-turn`, without `--wait`.
The message is durable immediately but becomes runnable when due, behind ordinary
input already queued then. Scheduling is a wakeup, not an exact wall-clock alarm:
capacity, suspend/resume, and clock changes can postpone execution. Delayed messages
keep their IDs but acquire a new queue position when promoted.

Managed agents can send a delayed message to themselves and end their turn. They
do not need to keep a turn open with sleep or polling.

## Files and images

Attach files to a startup message or a follow-up. The text body is still required:

```sh
hivectl spawn reviewer --cwd /absolute/repo --body-file review.md \
  --attach screenshots/failure.png --attach logs/trace.log --request-id review-files-1
hivectl send AGENT_ID --body-file question.md --attach ./diagram.png \
  --request-id diagram-1
hivectl send AGENT_ID --delay 10m --body-file reminder.md \
  --attach report.txt --request-id report-later-1
hivectl send AGENT_ID --wait --timeout 45 --body-file question.md \
  --attach result.json --request-id inspect-result-1
```

`--attach` is repeatable and ordered. Relative paths, including paths in a CLI
batch's `attachments` arrays, resolve against the **invoking directory**, not the
body file, operations file, or target agent's working directory. Hosted
`hive_spawn` / `hive_send`, their batch operations, and external
`hive_call` accept `attachments: [absolute paths]`:

```json
{"target":"REVIEWER_ID","body":"Explain this failure.","attachments":["/absolute/repo/failure.png","/absolute/repo/trace.log"],"request_id":"failure-1"}
```

| Limit | Behavior |
| --- | --- |
| Ten files per message | More files are rejected. |
| 20 MiB per copied file | Larger files become live path references. |
| 50 MiB copied per message | The first file exceeding the remaining budget and all following files become live references. A file individually over 20 MiB does not consume that budget. |

Copies are taken **at acceptance**, including scheduled messages. They retain the
exact original filename in a unique private folder; identical content and filename
share one stored copy. Changed or deleted sources do not affect accepted snapshots.
A retry with the same request ID and identical arguments returns the original
acceptance without reopening sources. Different ordered paths conflict; a fresh
request ID reads the files again. Keep the same invoking directory when retrying
relative CLI paths.

Files that exceed copy limits are clearly labeled **LIVE PATH** with canonical
path, observed size/time and identity, and the reason they were not copied. They
may change or disappear before the recipient reads them. Hive never deletes
these source files. Neither copies nor live references grant new sandbox access.

Readable regular files are accepted, including service-owned and shared-writable
logs. Symlinks are resolved to their canonical target while retaining the supplied
filename. Directories, devices, FIFOs, private Hive state, and multiply-linked
files owned by the current user are refused. Errors identify the offending path.
To forward a received snapshot, copy it into your workspace and attach that copy;
the private attachment store itself is not an allowed source.

Every attachment adds an escaped reference block to the message. PNG, JPEG, GIF
and WebP snapshots also become native image input for `next-turn` and `redirect`
when eligible; magic bytes take precedence over the extension. PDFs, text and
other files remain references for file tools. **Steering always carries references
only**, on both providers; there is no native-image steering flag.

Native images must decode successfully, be at most 2,000 pixels on either side,
and fit 4 MiB of base64-equivalent bytes each and 8 MiB per message. A conversation
has a conservative retained allowance of **20 image occurrences and 8 MiB**, which
branches inherit. Duplicate occurrences count separately even if storage is
shared. The allowance is charged once before submission and is not reclaimed by
auth changes, compaction, or a later reference fallback. Provider frame limits
also apply. Unsupported, malformed or over-budget images retain their reference
with a reason; Hive does not silently resize or truncate them.

`messages ID` shows each delivery's attachment metadata. `read ID` includes it in
acceptance and turn-start events: acceptance may say `pending`; turn-start records
the final native/reference choice. `status ID` reports `latest_attachments` for
the newest incoming message with attachments and `attachment_store_bytes` for the
whole controller's indexed snapshot store. These surfaces return metadata and
paths, never file contents. Older attachments remain available through `messages`.

Snapshots live with their message references, with no time-based expiry. Purge and
GC remove references and queue deletion only when the last reference is gone;
startup recovery finishes interrupted cleanup. Their `attachment_bytes` counts
unique bytes eligible for reclamation at that commit, not a promise of immediate
physical deletion if a concurrent request reuses the file. Live paths are excluded.
Attachments are **not automatically ingested into Comb**.

## Wait for results and external MCP

### Wait for the exact message you sent

```sh
hivectl send AGENT_ID --body-file question.md --request-id question-1 \
  --wait --timeout 45
# If observation times out, use the returned message_id; do not resend:
hivectl wait --message MESSAGE_ID --timeout 120
```

`send --wait` matches the **final output of the turn consuming that message**.
It does not wait for the whole task or a later answer assembled after child agents
reply. It supports `next-turn` and `redirect`, not `steer` or delayed input.

The CLI observes outside the daemon's writer. The default timeout is 45 seconds,
range 1–600, for both `send --wait` and `wait --message`. Timeout or caller disconnect
stops observation only; the message stays queued or running. Keep both IDs:
`request_id` retries the identical send; `message_id` resumes observation without
sending again.

| Outcome | Meaning |
| --- | --- |
| `replied` | The consuming turn finished and its retained output is available. |
| `timeout` | Observation ended before a terminal result. The response can include queue progress. |
| `uncertain` | Delivery/result could not be proved; it will not be replayed automatically. |
| `cancelled` | The message/turn was interrupted or made undeliverable. |
| `failed` | The consuming turn failed. |
| `observation_unavailable` | The observer could not obtain a reliable answer, for example after losing its daemon connection. Inspect the returned IDs/error before retrying. |

Only `replied` exits successfully for message waits. Results may include a large
output descriptor, null text for a turn with no final text, or explicitly marked
partial output. A compacted reply can produce `result_expired`; purging its agent
can make the message unknown. Neither means the original request should be replayed.

### Wait for an agent event

```sh
hivectl wait AGENT_ID --until completed --timeout 60
hivectl read AGENT_ID --limit 100
# Use an event cursor from read, rather than 0, to watch subsequent turns:
hivectl wait AGENT_ID --until turn-finished --after EVENT_CURSOR --timeout 120
```

These waits default to 60 seconds, range 1–3,600. `--until completed` is the default;
`--after` requires `--until turn-finished`, including an explicit zero cursor.
`--until turn-finished` observes a finished turn after the exclusive `--after`
cursor (default 0). An old cursor can match an old turn. Agent waits are not
message-correlated: inspect the returned event and task outcome, including failure,
interruption, cancellation, or lifetime expiry, rather than treating every wakeup
as a successful answer. `--message` cannot be combined with a target, `--until`,
or `--after`.

### Wait for a joined batch

```sh
hivectl wait --join JOIN_ID --timeout 45
hivectl read join:JOIN_ID
```

Use the `join.join_id` returned by `batch --join`. The wait observes all member
messages, with a default timeout of 45 seconds and a range of 1–600. It cannot be
combined with an agent target, `--message`, `--until`, or `--after`. A timeout
leaves collection running. See [joined batches](#collect-replies-with-a-join) for
acceptance, result states, exit codes, and partial delivery.

### Use from an outside MCP client

`hivectl mcp` serves newline-delimited MCP JSON-RPC on stdio and connects to an
already running daemon. Point an external client's MCP configuration at the release
binary, using the same state root:

```json
{
  "mcpServers": {
    "hive": {
      "command": "/absolute/path/to/hive",
      "args": ["--state-dir", "/absolute/private-state", "mcp"]
    }
  }
}
```

This server exposes exactly three tools:

| Tool | Arguments |
| --- | --- |
| `hive_call` | Required `target`, `body`, `request_id`; optional `attachments` (up to ten absolute paths), `mode` (`next-turn` or `redirect`) and `timeout` (default 45, max 600 seconds). Send and wait for that message. |
| `hive_wait_message` | Required `message_id`; optional `timeout`. Resume the same observation without resending. |
| `hive_output` | Required artifact `id`; optional byte `offset` (default 0), `limit` (4–65,536 bytes; default 65,536). |

For example, call `hive_call` with:

```json
{"target":"AGENT_ID","body":"Summarize the current diff.","request_id":"summary-1","timeout":45}
```

For a later observation, call `hive_wait_message` with:

```json
{"message_id":"MESSAGE_ID","timeout":45}
```

To page an output artifact, call `hive_output` with:

```json
{"id":"00000000-0000-4000-8000-000000000001","offset":0,"limit":65536}
```

Tool arguments must be an object. Invalid fields return `isError: true` with the
same `{ "error": ... }` in `structuredContent` and text. The error identifies a
`code`, `field`, `expected` form and complete `examples`; it never echoes supplied
values or private parser details. Field paths are relative to `tools/call` params,
for example `/arguments/timeout`. Correct the indicated field before retrying.
Malformed request structure or an unknown tool returns JSON-RPC `-32602` with
that diagnostic in `error.data`. Neither rejection starts an observer or sends a
message. Omitted arguments mean an empty object and report missing required fields.

Each MCP server permits at most 16 concurrent observers. Cancellation releases the
observer, not the accepted message. Client tools may have a shorter timeout of their
own; use bounded calls and resume with `hive_wait_message`.

These blocking interfaces are for **outside callers**. Managed agents receive
asynchronous replies that activate their next turn and should use the hosted tools
below. Holding an internal turn open while waiting wastes capacity and can deadlock
mutually waiting agents.

## Change agent options or login

Discover the supported settings without starting a provider:

```sh
hivectl options catalog
hivectl options catalog --provider codex --model gpt-6-luna
hivectl options catalog --provider claude --model claude-sonnet-4-6
```

The catalog reports types, accepted values and bounds, defaults, scopes, native
mappings, ownership and verified provider versions. A model filter uses recorded
capability rules; actual model/effort membership is checked against the selected
login's live catalog when spawning or changing options.

```sh
hivectl set-options AGENT_ID --option model=gpt-6-luna --option effort=low \
  --request-id settings-1
hivectl set-options AGENT_ID --option codex.reasoning_summary=detailed \
  --request-id settings-2
hivectl set-options CLAUDE_AGENT_ID --option model=claude-sonnet-4-6 \
  --option thinking=off --request-id settings-3
hivectl set-options AGENT_ID --option codex.reasoning_summary=null \
  --request-id settings-reset
hivectl set-auth AGENT_ID codex-backup --request-id login-change-1
hivectl set-auth AGENT_ID codex-pool --request-id login-change-2
```

`set-options TARGET [--option KEY=VALUE]... [--native-config FILE] [--request-id ID]`
persistently changes desired settings. Supply at least one option or a native
file. The call validates the complete resulting selection before accepting it.
It never interrupts a turn, adds a message, or resumes a paused agent. It takes
effect before the next submitted turn. A successful response contains `desired`
and per-key `takes_effect`: `next_turn`, `next_session` or `process_move`.
`options_revision` identifies the desired selection; `applied_options_revision`
advances only after native turn acceptance. Status also retains reserved, loaded
session and last accepted selections, so a pending change is visible.

| Key | Provider | Values | Scope |
| --- | --- | --- | --- |
| `model` | Both | Model token advertised by the login | Codex turn; Claude session |
| `effort` | Both | Effort advertised by the selected model | Codex turn; Claude session |
| `thinking` | Claude | `on`, `off`, or integer token budget 1,024–31,000 on recorded supporting models | Session |
| `codex.reasoning_summary` | Codex | `auto`, `concise`, `detailed`, `none` | Turn |
| `codex.verbosity` | Codex | `low`, `medium`, `high` on recorded GPT-5 models | Session |
| `codex.auto_compact_token_limit` | Codex | Integer 1,024–2,000,000 | Session |
| `codex.tool_output_token_limit` | Codex | Integer 128–1,000,000 | Session |
| `codex.request_compression` | Codex | Boolean | Process |
| `claude.auto_compact_window` | Claude | Integer 100,000–1,000,000 | Session |
| `claude.max_output_tokens` | Claude | Integer 1,024–128,000 | Session |
| `claude.max_mcp_output_tokens` | Claude | Integer 128–1,000,000 | Session |
| `claude.skills` | Claude | Boolean: bundled skills through the `Skill` tool | Session |

The catalog is authoritative for bounds and model restrictions. Claude thinking
budgets must be below any explicit output cap. Unknown or unsupported model
capabilities are refused. Codex has no separate `thinking` toggle; choose effort.
Requested token caps remain subject to native model limits. Optional settings
require a provider version listed in the catalog; unsupported versions fail
before input and leave the desired revision pending.

A turn setting is sent on the next turn; a session setting resumes history with
new session configuration; a process setting moves Codex to a process with the
same effective process settings. All scopes change at one turn boundary. Failed
application keeps the revision pending and does not replay uncertain input.

### Provider skills

Claude agents have no skills unless `claude.skills` is true. With it, Claude
starts with its slash commands and its `Skill` tool, so the agent can use the
skills bundled with Claude Code, such as `code-review`, `simplify` or `debug`,
when a message asks for one ("use the code-review skill on the current diff").
Hive's tool policy still applies to every tool a skill uses: a read-only agent's
skill can run only checked read-only `git` commands and cannot write files. Skills that run in a forked subagent
work the same way, but a forked skill cannot start another skill, and background
skills are refused. Skills from the repository or your user folder are not
loaded, because Hive starts Claude without your settings.

```yaml
roles:
  reviewer:
    provider: claude
    options: {claude.skills: true}
```

Codex agents load Codex's own skills by default: its system skills and the
repository's `.codex/skills` and `.agents/skills` below the agent's directory.
Codex has no switch to turn them all off.

### Presets, inheritance and reset

Roles are presets. Provider defaults and roles accept typed `options`; a role can
also name a native configuration file:

```yaml
providers:
  codex:
    auth: codex-main
    model: gpt-6-luna
    options: {effort: low}
roles:
  reviewer:
    options: {codex.reasoning_summary: detailed}
    config: reviewer.toml
```

```sh
hivectl spawn --preset reviewer --cwd . --body 'Review the change.' \
  --option effort=high --option codex.request_compression=true
```

Precedence, highest first: later mutations, explicit spawn settings, role,
same-provider parent's effective settings, provider defaults, catalog defaults.
Branch inheritance uses the source's effective settings. Pending desired changes
are not inherited. Resolution freezes a spawn baseline, including explicit spawn
values. Later `null` resets return to that baseline, never today's role file or
parent state. Spawn `null` removes an optional lower-layer setting; required
model/effort cannot be cleared. Role/provider maps reject null.

Repeating a canonical key through a shortcut, typed option or native overlay in
one layer is an error, even when the values agree. A positional role and
`--preset` also conflict. Each agent accepts at most 64 typed keys.

### Native overlays

```sh
# reviewer.toml: tool_output_token_limit = 4096
hivectl spawn --preset reviewer --cwd . --body 'Review the change.' \
  --native-config reviewer.toml
hivectl set-options AGENT_ID --native-config reviewer.toml --request-id native-1
```

Codex files use strict TOML; Claude files use a strict JSON settings object, for
example `{"alwaysThinkingEnabled":false,"autoCompactWindow":200000}`. Each file
is UTF-8 and at most 64 KiB. Role paths resolve relative to the main configuration.
The bytes are read once per spawn and stored privately with SHA-256. Editing the
file does not change an existing agent. Tool `config` is inline text, never a path.

Only catalog-listed native keys are accepted. Native `model` and effort keys
are refused: use the typed inputs. Sandbox, approvals, MCP/tools, hooks, shell
rules, history paths, credentials, endpoints and arbitrary environment injection
are owned by Hive and cannot be passed through. Use the corresponding Hive
configuration or auth interface. Unknown keys and duplicate JSON keys are errors.
The resolved settings follow the same scope routing as typed options.

Omitting a mutation's config retains its current overlay; supplying config
replaces that mutation overlay. Empty Codex TOML or Claude `{}` clears it. A later
typed mutation replaces the same native key; a null reset removes either mutation
form and reveals the frozen baseline. Baseline overlays remain beneath them.
Public status/events redact native-derived values and expose kind, hash, source
and key list only. Explicit typed values remain visible.
Provenance includes `native_overlay: true` for native-derived values; an omitted
flag means false.

Process variants share by provider, login and canonical process-setting values;
formatting or typed/native spelling does not split a process. The default empty
setting set keeps normal sharing. Logical variants are bounded by
`runtime.max_process_variants`; spawn/change fails atomically when over limit.
The physical pool also counts warm, starting, draining and unreaped variants and
waits for old capacity release before launching a new key. A fallback destination
without variant capacity leaves the binding unchanged and exposes
`process_variant_limit`; resume after capacity changes.

`set-auth TARGET PROFILE_OR_GROUP [--request-id ID]` likewise applies at a turn
boundary and requires the same provider. Switching login keeps the agent ID,
mailbox, children, and approvals, but starts a **fresh native conversation with a
bounded handoff**. It does not copy the entire old native history across accounts.
Pending uncertain input is never silently replayed.

### Login groups

A group chains or spreads agents across several logins of one provider. Add the
subscription profiles and a group to the daemon configuration:

```yaml
auth_profiles:
  codex-main: {provider: codex, method: subscription}
  codex-backup: {provider: codex, method: subscription}
  codex-team: {provider: codex, method: subscription}
auth_groups:
  codex-chain:
    mode: ordered
    members: [codex-main, codex-backup]
  codex-pool:
    mode: balanced
    members:
      - codex-main                        # weight 1
      - {profile: codex-team, weight: 3}  # about three times as many agents
```

Authenticate each profile separately, restart the daemon to load the configuration,
then select a group with `spawn --auth codex-pool`, a role's `auth`, or `set-auth`:

```sh
hivectl --config hive.yaml auth login codex-backup
hivectl auth groups
hivectl agents --auth codex-main
hivectl status AGENT_ID
```

`mode` is required. `members` lists one or more subscription profiles, and the
optional `paid_tail` lists API-key profiles (below). A profile appears at most once
per group, and all of a group's profiles use one provider. There is no shorthand
list form.

- **Ordered** groups use the first usable member and advance down the list when it
  is limited. Ordered members cannot set `weight`.
- **Balanced** groups give each new agent the usable member with the lowest
  `(bound agents + active turns + 1) / weight`, breaking ties by member order. A
  member without `weight` weighs 1, so equal weights rotate like round robin.
  Bound agents are the login's nonterminal agents and active turns are its turns
  in progress. Both are read from durable state as each agent is assigned, so a
  batch of spawns or a daemon restart continues the same split. A member that has
  no room in `runtime.max_process_variants` for the agent's process settings is
  skipped. If no usable member has room, the request fails with
  `process_variant_limit`. Capacity never authorizes the paid tail.

Switching login starts a fresh native conversation and loses the provider's prompt
cache, so an agent stays on its login while that login is usable. When a login is
limited, agents in an ordered group move to the next usable member at their next
turn boundary. In a balanced group an agent moves only when it has input to
deliver, including the continuation of a cut-off turn. Each move counts toward the
destination's load at once, so the agents of a limited login spread across the
other members. Idle agents keep their login and conversation; their next input
moves them if the login is still limited. Agents are not moved back when a login
recovers. New spawns and later moves prefer it because its load is lowest. Branches
stay on their source's login.

If every member is limited, agents park until a reset/probe window, preserving
queued input. Unknown reset times use a bounded retry window; profiles are not
hammered by one probe per waiting agent. A single profile stays pinned unless you
explicitly change it. `set-auth` can also detach from a group and request a
particular profile. Auth status/login changes notify the daemon so waiting agents
can be reconsidered. Codex agents on one login still share processes up to
`sessions_per_codex_process`, so spreading agents over more logins can leave more
partly filled Codex processes.

`auth groups` lists each group's `mode`, its `members` and `paid_tail` with
`bound_agents`, `active_turns` and availability, and each balanced member's
`weight`. Agent `status` shows `auth_group` and `auth_decision`, the last selection
as `{mode, reason}`:

| `reason` | Meaning |
| --- | --- |
| `ordered`, `balanced` | The group's rule chose this subscription. |
| `sticky` | The agent kept its current login, or a branch kept its source's login. |
| `limited_move` | The agent left a limited or failed subscription for this one. |
| `paid_tail` | No subscription was usable and the agent allows API fallback. |
| `waiting` | Every member was limited; the agent waits with its input. |

Agents on a direct profile have no decision. `spawned` and `auth_switched` events
carry the same `auth_group` and `auth_decision` fields.

To permit paid fallback, register an API profile and place it in the group's
`paid_tail`, which is always used in order and never balanced:

```yaml
auth_profiles:
  codex-main: {provider: codex, method: subscription}
  codex-api: {provider: codex, method: api_key, api_key_env: OPENAI_API_KEY}
auth_groups:
  codex-pool:
    mode: ordered
    members: [codex-main]
    paid_tail: [codex-api]
```

All members must use one provider. Automatic paid fallback requires positive usage
exhaustion evidence for every subscription member and a compatible model/effort.
An unknown, missing or failed subscription credential does not authorize API
spending. Select `spawn --no-api-fallback`, or `api_fallback: false` in a role, to
skip a group's API members. A direct `--auth codex-api` remains an explicit paid
choice. Groups require at least one subscription member.

An agent using the API tail returns at a turn boundary only to a model-compatible
subscription whose availability has been confirmed. A successful subscription
turn or authoritative subscription quota observation can supply confirmation;
the reset clock or a successful login alone does not. There is no background
throwaway inference to probe API keys. Switching profiles uses the fresh native
conversation and bounded handoff described above; rotating a key within the same
profile preserves its native conversation. Old-key callbacks cannot clear a
new-key hold. Inspect `auth groups` and agent `status` for the selected profile,
`billing`, waits and pending switches.

## Agent names

A name is a label for people, such as `diff reviewer`. It is not unique and never
selects an agent: commands, hosted tools and `@parent`/`@child` still take agent
IDs. Give one at spawn time with `--name` (or `name` in a batch or hosted spawn),
and change or remove it at any time, also after the agent has ended:

```sh
hivectl spawn reviewer --name "diff reviewer" --cwd /absolute/repo --body-file review.md
hivectl set-name AGENT_ID reviewer --request-id rename-1
hivectl set-name AGENT_ID --clear --request-id rename-2
```

A name is 1–64 UTF-8 bytes, without control characters or leading or trailing
spaces; other names are refused with `invalid_value`. `set-name` takes effect at
once and does not touch the conversation, so agents keep the identity they were
started with. Its request is `{"op":"set_name","target":ID,"name":"reviewer"}`, or
`"clear":true` instead of `name`, and it replies with `agent_id` and `name`.

Named agents show their name in `status`, `agents` and `attach` (`name`), in
`hivectl approvals` (`agent_name`), and in `hivectl tui`, `hivectl attach` and `hivectl panes`
labels. An unnamed agent has no `name` key and keeps its usual role and ID labels.

## Human questions and approvals

Managed agents ask through `hive_ask`. There is deliberately no CLI `ask`:
questions belong to a running agent turn. A question can be a plain confirmation
or a small structured form. For example, an agent calls:

```json
{
  "message": "Which test suite should I run before continuing?",
  "schema": {
    "type": "object",
    "properties": {"suite": {"type": "string", "enum": ["focused", "full"]}},
    "required": ["suite"]
  },
  "request_id": "test-scope-1"
}
```

The tool returns an `approval_id` immediately. The agent should **end its turn**,
not complete its task. The human's response arrives as new input. There is at most
one pending question per agent: a new ask supersedes it. Completion, termination,
and lifetime expiry withdraw pending questions. Other messages can still reach an
agent with a pending question; an answer does not interrupt a running turn.

### List and inspect

```sh
hivectl approvals
hivectl approvals --agent AGENT_ID --limit 100
hivectl approvals --all --limit 100
hivectl approvals --all --after "$APPROVAL_CURSOR"
hivectl approvals --wait --timeout 60
hivectl approvals show 00000000-0000-4000-8000-000000000001
```

Without `--all`, list only pending questions. `--agent` filters by agent.
`--limit` defaults to 100, range 1–200. Pass the returned JSON `next_after` to
`--after` for another page. Each question carries its agent's `agent_id`, `role`
and `agent_name`. The cursor is an object with `created_at` (integer from
0 to 9,223,372,036,854,775,807) and `id` (nonempty string, at most 128 UTF-8 bytes).
Malformed cursors are rejected before startup or connection; their contents are
never echoed in errors. `--wait` waits for a pending question, with timeout
60 seconds by default, range 1–3,600; it cannot be combined with `--all` or `--after`.
`show` returns one question, including its schema and current status. Approval IDs
are UUIDs. Input rejections exit 2; polling timeout exits 1 and changes no questions.

### Answer

```sh
# Accept a plain confirmation:
hivectl respond 00000000-0000-4000-8000-000000000001 --accept --request-id answer-1
# Accept a form, with values conforming to its schema:
hivectl respond APPROVAL_ID --accept --data '{"suite":"focused"}' \
  --request-id answer-2
hivectl respond APPROVAL_ID --accept --data-file answer.json --request-id answer-3
# Decline, optionally explaining why, or dismiss without a decision:
hivectl respond APPROVAL_ID --decline --reason 'Wait for the release window.' \
  --request-id answer-4
hivectl respond APPROVAL_ID --cancel --request-id answer-5
```

Choose exactly one of `--accept`, `--decline`, or `--cancel`. `--data` and
`--data-file` are mutually exclusive and only apply to acceptance; `--reason`
only applies to decline. Form values are JSON, not a quoted string containing JSON.
Required values must be supplied; schema defaults are not automatically applied.
Data is limited to 61,440 serialized JSON bytes and decline reasons to 61,440 UTF-8
bytes. The complete formatted answer is limited to 65,536 bytes. Input errors name
the selected flag; `--data-file` paths appear only in local stderr, never JSON.
An invalid response leaves the pending question and mailbox unchanged.
A response to a stale/superseded question is rejected. Retrying an identical response
with its request ID returns the original result.

Ask requires `message` (up to 16 KiB) and `request_id`. Optional fields are
`title` (256 bytes), `schema` (16 KiB, flat object, at most 16 fields and 64 choices
per field), and `destructive` (a presentation hint). Form fields can use supported
primitive types and choice lists;
see [form schemas](#approval-form-schemas). Never request secrets in a form.

Approvals are **advisory application decisions**, not OS/provider execution
permissions. Use Hive for the agent's questions to the human, while respecting
host tool restrictions and existing task authorization. An agent must not use the
human-facing `respond` command to approve its own action.

### Approval form schemas

The schema is a flat object with `type: object`, `properties` and an optional
`required` list. Each property may have `title`, `description` and `default`.
Defaults are validated but are not automatically supplied when answering.

| Field | Supported schema |
| --- | --- |
| Text | `type: string`; optional `minLength`, `maxLength`, `format` (`email`, `uri`, `date`, `date-time`) |
| Number | `type: number` or `integer`; optional `minimum`, `maximum` |
| Yes/no | `type: boolean` |
| Single choice | `type: string` with `enum`, or `oneOf: [{"const":"value","title":"Label"}]` |
| Multiple choice | `type: array` with `items: {"type":"string","enum":[...]}` or `items: {"anyOf":[{"const":"value","title":"Label"}]}`; optional `minItems`, `maxItems` |

For example, pass this object as the `schema` field of `hive_ask`:

```json
{"type":"object","properties":{"suite":{"type":"string","enum":["focused","full"]}},"required":["suite"]}
```

Accept it with `hivectl respond APPROVAL_ID --accept --data '{"suite":"focused"}'`.
Nested objects, arrays of objects, `$ref`, `pattern` and unknown keywords are
rejected. Forms allow at most 16 properties and 64 options per choice. The schema
and question are each limited to 16 KiB; response data is limited to 60 KiB.
Invalid schemas return `invalid_value` with `/schema`, its expected form and a
complete example call; asking outside an active provider
turn returns `no_active_turn`. Forms must never request passwords, tokens or API keys.

## Stop, resume, interrupt, complete, terminate

| Command | Effect |
| --- | --- |
| `stop ID [--request-id ID]` | Stop current execution and pause automatic activation, retaining queued input and conversation for later resume. A pending unbound branch is the exception described above. |
| `resume ID [--request-id ID]` | Remove the manual pause and resume the existing conversation when runnable. Recovery fences or a permanently ended task still prevent execution. |
| `interrupt ID [--request-id ID]` | Interrupt the current turn without permanently pausing the agent. Queued input may start another turn. |
| `complete ID (--body TEXT \| --result-file FILE) [--request-id ID]` | Request successful task completion with a result. For a running turn, completion commits only at a successful turn boundary. It cancels the agent's open joins and expires its `with_parent` children. |
| `terminate ID [--request-id ID]` | Permanently cancel the task, make queued input undeliverable, and request asynchronous session cleanup. It cannot resume. |

```sh
hivectl stop AGENT_ID
hivectl status AGENT_ID
hivectl resume AGENT_ID
hivectl interrupt AGENT_ID
hivectl complete AGENT_ID --result-file result.md --request-id finished-1
hivectl terminate OTHER_AGENT_ID --request-id cancel-1
```

Terminating or expiring an agent releases its conversation after asynchronous
cleanup. A shared Codex process can remain alive for other conversations; an empty
process may linger briefly before exiting, according to `linger_seconds`. Claude uses one process per
conversation. An accepted termination request is not proof of process exit.

`complete` requires exactly one of `--body TEXT` or `--result-file FILE`.
Both accept 1–65,536 UTF-8 bytes and preserve whitespace:

```sh
hivectl complete AGENT_ID --body "Implemented and reviewed." --request-id finished-1
```

All five controls accept `--request-id ID`. Reuse the same ID and arguments when
retrying a request whose acknowledgement was lost. A successful request is saved:
its retry returns the original acknowledgement without applying the control again,
even if the agent has since started another turn. Reusing an accepted ID with a
different command or target returns `request_conflict`. Rejected requests are not
saved and may succeed if retried after the agent's state changes. Omitting the
option generates a new ID for each CLI invocation. An acknowledgement reports
acceptance, not completion of asynchronous shutdown or interruption.

These examples are alternative controls, not a required sequence. Termination
acceptance does not mean every provider process has already exited; inspect status
before cleanup. Terminating an already ended agent is rejected, except a replay of
the identical accepted request. Completion notifies the parent. Ordinary final
assistant output does **not** automatically send a message to the parent: a reusable
reviewer should explicitly send its findings and end its turn.

### Lifetimes

- `independent` is the default: the agent survives its parent's end.
- `with_parent` expires when its parent completes, terminates, or expires.
  Expiry cascades transitively through bound children and cancels their queued input.
  An independent child breaks the chain; it and its subtree survive that cascade.

Stopping, pausing, failing, or merely ending a turn does not trigger the cascade.
A failure can be resumed. The cascade is committed atomically with the permanent
parent outcome; session cleanup follows asynchronously. Human spawns have no
parent and cannot use `with_parent`. A hosted agent can select it in `hive_spawn`.
Parent links are lifetime/delegation metadata, not visibility restrictions.

## Batch operations

```sh
hivectl batch --operations-file operations.json --request-id review-batch-1
```

The file is a JSON array of **1–64** spawn, send, or terminate operations, at most
512 KiB. The complete serialized request also has a 512 KiB cap, including the
batch wrapper, join options, unknown inner fields and resolved attachment paths.
Each operation requires its own `request_id` (1–128 UTF-8 bytes, no control
characters). Invalid headers name their zero-based JSON pointer, such as
`/operations/2/request_id`, alongside `--operations-file`. The file is read once;
its path appears only in local stderr diagnostics. No batch data is echoed. The outer `--request-id`
identifies the batch. Acceptance is atomic: a validation error rolls back the batch;
provider execution after acceptance is asynchronous and may succeed or fail
independently.

An `operations.json` example sends two independent review requests:

```json
[
  {"op":"send","target":"REVIEWER_A","body":"Review correctness. Send findings to IMPLEMENTOR_ID.","request_id":"review-a-1"},
  {"op":"send","target":"REVIEWER_B","body":"Review error handling. Send findings to IMPLEMENTOR_ID.","request_id":"review-b-1"}
]
```

JSON uses `body` and `instructions` text, not file paths; branching uses
`from_agent`, not `--from`; scheduling uses integer `delay_seconds`. For example:

```json
[
  {"op":"spawn","role":"reviewer","name":"diff reviewer","cwd":"/absolute/path/to/repo","body":"Review the current diff.","request_id":"batch-reviewer-1"},
  {"op":"send","target":"AGENT_ID","body":"Check progress.","delay_seconds":60,"request_id":"batch-reminder-1"},
  {"op":"terminate","target":"FINISHED_REVIEWER_ID","request_id":"batch-stop-1"}
]
```

The terminate target must still be an open task; a reviewer that has already
explicitly completed cannot be terminated again. Spawn accepts the same settings
as the CLI (`api_fallback` is a boolean rather than `--no-api-fallback`). A later
operation cannot reference a new spawn by a made-up alias: use the returned agent
ID in a subsequent request. Batches do not support blocking calls, approvals,
model/auth changes, or purge.

### Collect replies with a join

Use `--join` when you want the answers to a batch together. Ask each worker for its
answer as final output, without a separate send back:

```json
[
  {"op":"send","target":"REVIEWER_A","body":"Review correctness. Return findings as your final answer; stay available for another round.","request_id":"review-a-2"},
  {"op":"send","target":"REVIEWER_B","body":"Review error handling. Return findings as your final answer; stay available for another round.","request_id":"review-b-2"}
]
```

```sh
# Add --deadline-seconds 1800 to this command for optional partial delivery.
hivectl batch --operations-file operations.json --request-id review-round-2 --join
hivectl wait --join JOIN_ID --timeout 45
hivectl read join:JOIN_ID
```

Use fresh operation IDs for each new batch; reuse the exact
outer ID and arguments only to retry acceptance. The response contains `results`
and `join: {join_id, members}`, with message IDs in operation order. `--deadline-seconds`
requires `--join`; there is no default deadline. Seconds must be positive, and
current Unix time plus the delay must fit 9223372036854775807. The CLI checks this
before reading the file, and the daemon checks again before acceptance. Hosted callers use the same options
as `hive_batch({operations: [...], join: {}, request_id: "round"})`, or
`join: {deadline_seconds: 1800}`.

Every send and spawn startup is a member, including a branch spawn. Termination
operations contribute no reply, and an all-termination join is invalid. Several
members may target the same agent. Member sends require `next-turn`, permit
`delay_seconds`, and cannot target their requester. `steer` and `redirect` are
rejected. A new joined batch requires fresh, distinct operation IDs, also distinct
from its outer ID; it cannot adopt an earlier message.

A reply is the final output of **the turn that consumed that message**, preferring
final-answer text, otherwise the last assistant message. Outcomes are `completed`,
`failed`, `interrupted`, `undeliverable`, or `uncertain`; incomplete members are
`pending`. Interrupted or failed replies can contain partial text. Automatic
login-fallback continuations are separate messages/turns: they do not replace an
interrupted result. If the member turn also completes its task, `task_completed`
retains its completion summary. When the requester is that worker's parent, the
join replaces that turn's usual child-completion notification. An explicit send
back remains a separate message.

A hosted requester ends its turn after acceptance, keeping its task open. Once
all members finish it receives one new `join_result` input on its existing
conversation. At a configured deadline it first receives finished answers plus
pending markers; unfinished text is withheld. Collection continues, and the final
input contains only members whose results have not already been delivered. There
are at most two inputs, and each finished answer is delivered once. Human joins
have no mailbox: `read join:ID` returns the snapshot and the deadline changes its
state to `deadline_passed` while collection continues.

`wait --join` is an external observer with a default 45-second timeout and a range
of 1–600 seconds. It returns the snapshot when completed or cancelled. A completed
join exits successfully even if some members failed; inspect each member's outcome.
A cancelled join exits unsuccessfully. On timeout, JSON contains `wait: "timeout"`,
`join_id`, and the last observed `join` snapshot (or null before any observation);
exit status is nonzero. Repeat the wait to resume observation. Timeout, disconnect,
and Ctrl-C do not cancel accepted work. `read --follow` accepts agent targets only,
not join targets. Hosted agents should yield instead of blocking or polling.

A join ID must still be retained. An unknown ID in `read join:ID` reports an input
error for `TARGET`; an initially unknown `wait --join ID` reports `--join`. Both
exit 2 with JSON and a human-readable diagnostic. If a join disappears after a
successful wait observation, the result is `observation_unavailable` and retains
the last snapshot. Database or transport failures also remain observation failures.

The snapshot has `join_id`, `requester`, `request_id`, Unix-second `created_at` and
optional `deadline`, `state`, and ordered `members`. State is `collecting`,
`deadline_passed`, `completed`, or `cancelled`. Each member identifies its zero-based
`position`, `message_id`, and `target`, plus its outcome. `output` and
`task_completed` are null or objects with `text` and an optional `read_target`.
`output_selection` is `final_answer`, `last_assistant_message`, or `no_text`;
`output_partial` identifies a member that has not completed successfully.
`delivered` records inclusion in a hosted requester's result message, so it remains
false for human joins. A pending snapshot may show provisional text; that text is
withheld from automatic deadline delivery until the member finishes.

If observation fails, the CLI returns nonzero with `wait: "observation_unavailable"`,
the join ID, its last snapshot, and an error. Restore the daemon connection and
repeat the wait with the same ID; observation failure does not resend work.

Each member has a shared 64 KiB UTF-8 inline budget for selected text and any task
completion summary. Overflow has an `artifact:ID` read target for full output.
Targets in provisional output expire when newer output replaces them; compaction
reclaims those superseded files. Worker-owned event artifacts are retained separately.
The combined input can approach 4 MiB plus headers (and a model's context limit).
Results survive worker purge and call-result compaction. A requester completing,
terminating, expiring, or being purged cancels collection without cancelling
independent workers. A hosted requester therefore cannot complete while its join
is collecting before any deadline, or while a join result is still undelivered:
`hive_complete` returns `join_open`, and it should end its turn instead. After
a deadline's partial result is delivered, it may complete and stop waiting. Starting a joined batch
after requesting completion returns `completion_pending`. A human `complete`
still applies, and cancels the collection. Stop, pause and failure do not cancel it. Requester purge/GC
removes its join snapshots and owned output; human joins remain in the state
directory until it is removed. Agent status lists `open_joins`, and open joins count
as waiting work in stalled-group reports.

## Purge, garbage collection, and compaction

These have different scopes. `purge` deletes selected agents, `gc` applies an age
policy to ended agents, and `compact` prunes diagnostic/result history while keeping
agent records.

### Delete an ended agent

```sh
hivectl purge AGENT_ID --dry-run
hivectl purge AGENT_ID --request-id purge-1
hivectl purge ROOT_ID --recursive --dry-run
hivectl purge ROOT_ID --recursive --request-id purge-tree-1
```

Purge requires permanent completion, cancellation, or lifetime expiry and confirmed
release of provider/recovery ownership. A stopped or failed-but-resumable agent is
not eligible. `--dry-run` previews the selection and blockers without deleting.
`--recursive` selects **all descendants**, including independent children; without
it, only the target is selected. This differs from lifetime termination cascades.
An active descendant blocks a recursive purge rather than being terminated for you.

Purge removes the selected agents' Hive records, owned output, and attachment
references. Last-reference snapshots are queued for cleanup. It retains
native Codex/Claude conversation files, login credentials, workspace files, other
agents' copies of messages, and shared recovery evidence. Minimal lineage/deduplication
evidence can remain. Purging a parent alone does not purge its children; retained
lineage allows later recursive cleanup even if the root record has gone.

Database deletion and output-file cleanup are separate steps. An error with code
`purge_cleanup_pending` means logical deletion committed but files still need cleanup;
retry the identical request ID or let startup recovery finish the saved cleanup.
Purge is a human/external CLI operation, not a hosted agent tool or batch operation.

### Collect old ended agents

```sh
hivectl gc --older-than 30d --dry-run
hivectl gc --older-than 30d
```

`--older-than` is required: a positive integer followed by `s`, `m`, `h`, `d`, or
`w`. The cutoff uses **permanent end time**, not creation or last activity. An idle
live agent is never deleted simply because it has not run lately.

GC applies the same safe-release checks as purge, skips blocked candidates, and
reports progress, blockers, and cleanup status. Each agent must qualify by its own
age; GC does not recursively delete younger children. The invocation uses a fixed
cutoff and commits bounded pages, so an interrupted run may have deleted earlier
pages. Inspect the result and rerun as needed. It is an explicit pass, not an
automatic background retention policy.

### Compact retained history

```sh
hivectl daemon stop
# Example cutoff is a Unix timestamp in seconds; choose one for your retention policy.
hivectl compact --before 1700000000 --keep-per-agent 100 --dry-run
hivectl compact --before 1790812800 --keep-per-agent 100
hivectl --config hive.yaml daemon start
```

`--before` is required and accepts Unix seconds from 0 through 9223372036854775807.
Only events strictly older than the cutoff are eligible; 0 selects no new old history.
`--keep-per-agent` defaults to 100 recent events and accepts 1 through
9223372036854775807. Zero and oversized values are rejected.

`--dry-run` requires the daemon stopped and reports `events_to_remove`,
`call_results_to_remove`, `orphan_artifacts_to_remove` and a separate
`pending_artifact_deletions` count. It does not recover attachments, drain pending
deletions, prune history, rewrite the output manifest or vacuum the database.
`attachment_recovery` is `not_evaluated`; recovery and ownership checks still run
when compaction is applied. The preview does not estimate resulting disk size.
Normal exclusive-lock and SQLite read coordination may touch their sidecar files.

Compaction
requires the daemon stopped; it prunes eligible older diagnostics and completed
message-result observations while retaining current state, delivery/deduplication
and recovery evidence. It does not purge agents or their retained large-output
files. Old event cursors can report expired history, and old message observations
can report `result_expired`. Consumers must handle those outcomes instead of
resubmitting work. Purge/GC release agent-owned artifacts when the agent is removed.

## Hosted agent tools

An agent **hosted by Hive** receives identity-bound tools automatically:

`hive_send` and `hive_terminate` also accept relative targets, including inside
`hive_batch`:

- `@parent`: the agent that spawned you, including when your conversation was
  branched from a different agent.
- `@child`: your sole retained direct child. Completed, terminated and expired
  children still count. If there are zero or multiple children, the request fails;
  list children with `hive_agents` using your ID as `parent`, then use an exact ID.

For example, a reviewer can send
`{"target":"@parent","body":"No findings.","request_id":"review-result-1"}`
without copying its parent's ID. A parent with one child can use
`{"target":"@child","request_id":"stop-child-1"}` with `hive_terminate`.
Normal lifecycle checks still apply: a relative target cannot revive an ended
agent. Resolution happens at acceptance; messages, events and replies retain the
canonical ID. Retrying an accepted request with the same `request_id` returns its
original result even if the child set has changed. Purged children no longer count
for new requests. External CLI/MCP callers have no relative context and must use
IDs; other hosted tools also continue to require IDs. Unknown IDs are never
automatically corrected or routed to a different agent.

| Tool | Purpose and example arguments |
| --- | --- |
| `hive_spawn` | Create a child; `{"role":"reviewer","body":"Review and send findings to PARENT_ID.","lifetime":"with_parent","request_id":"reviewer-1"}`. Use `from_agent` for a branch. Hosted ordinary spawns inherit the caller's directory unless overridden. |
| `hive_send` | Async input; `{"target":"AGENT_ID","body":"Review the fix.","request_id":"review-2"}`. Optional `mode` and `delay_seconds`. |
| `hive_batch` | Atomic acceptance; `{"operations":[...],"request_id":"batch-1"}`. Add `"join":{}` to collect replies, then end your turn; see [joins](#collect-replies-with-a-join). |
| `hive_set_options` | Change future settings; `{"target":"AGENT_ID","options":{"model":"sonnet","effort":"low"},"request_id":"options-1"}`. Optional `config` contains native overlay text. |
| `hive_options_catalog` | Read supported settings; `{"provider":"claude","model":"claude-sonnet-4-6"}`. Both filters are optional. |
| `hive_set_auth` | Change future login/group; `{"target":"AGENT_ID","auth":"claude-backup","request_id":"auth-1"}`. |
| `hive_set_name` | Change or remove a display name at once; `{"target":"AGENT_ID","name":"reviewer","request_id":"name-1"}`, or `"clear":true`. `hive_spawn` also takes `name`. |
| `hive_ask` | Ask the human; `{"message":"May I publish this reviewed change?","request_id":"publish-1"}`. |
| `hive_evidence` | Submit explicitly public review observations to configured collectors; see [shared learning evidence](#shared-learning-evidence). |
| `hive_complete` | Complete the caller's own task; `{"body":"Implemented and validated with ...","request_id":"done-1"}`. Refused with `join_open` while a join the caller requested is collecting before its deadline, or a join result is undelivered. |
| `hive_terminate` | Permanently cancel a target; `{"target":"AGENT_ID","request_id":"stop-1"}`. |
| `hive_status` | Inspect state; `{"target":"AGENT_ID"}`. |
| `hive_agents` | List agents; `{"parent":"PARENT_ID","limit":100}`. Stalled mode uses `stalled_seconds` and object `stalled_after`. |
| `hive_read` | Read events; `{"target":"AGENT_ID","after":0,"limit":100}`; also reads an artifact via its `read_target`. |
| `hive_messages` | Read mailbox history; `{"target":"AGENT_ID","limit":100}`; older pages use `before`. |

Mutating tools require stable `request_id` values. Their JSON fields match the
concepts in the CLI, with inline bodies instead of body files. These 15 hosted coordination tools
are **not** the three-tool external `mcp` surface. In particular, there is no hosted
blocking wait or human-response tool.

Invalid hosted-tool arguments return `code`, `message`, `field`, `expected` and
`examples`. Fields identify known arguments, for example `/request_id` or
`/operations/2/target`. Unknown properties identify their containing object, so
private property names are not echoed. Values, message bodies and file paths are
never included in these diagnostics. Tool descriptions include complete example
calls and each property explains its purpose and limits. Spawn/send tools accept
up to ten absolute attachment paths; files are captured only after the existing
request and ownership checks.


## Shared learning evidence

`evidence` deliberately shares public observations with configured Comb collectors.
It does not require a Comb tool binding on the submitting agent.

```sh
hivectl evidence review --summary 'Negative inputs lack validation.' \
  --verdict findings --file factorial.py --commit abc1234 --request-id review-42
hivectl evidence review --summary-file review-summary.txt \
  --verdict no_findings --file factorial.py --request-id review-43
hivectl evidence correction --summary 'The documented default is 128.' \
  --file docs/cli-reference.md --request-id correction-1
```

Provide `--summary` or `--summary-file`; summaries must be nonblank UTF-8 text,
at most 8,192 bytes, without NUL or private markers. `--file` repeats up to 64
nonempty public relative labels, each at most 512 UTF-8 bytes. These labels are
not opened or checked for existence. Empty or dot path components, controls,
backslashes, colons, leading tildes and private markers are rejected. `--commit` accepts 7–64 hexadecimal
characters. Reviews require `--verdict findings|no_findings`; corrections are human-only
and omit verdict. Private markers and absolute/traversing file paths are rejected.
Field errors identify the selected CLI flag (or JSON pointer for hosted tools).
Option errors are checked before reading a summary file; file paths appear only
in the local human diagnostic, never in the JSON error.
Text is explicitly shared by the caller; this is not a universal secret detector.
The daemon supplies the agent, role, execution and turn. Admission returns an event
ID and `queued`, `full`, `disabled` or `not_selected` for each retained destination.
Reuse `--request-id` to retry identical admission without a second event; changing
its arguments is a conflict. An admission acknowledgement is not a delivery receipt.

The native agent tool is `hive_evidence` with `kind: review`, `summary`, optional
`files`/`commit`, `verdict: findings|no_findings`, and `request_id`. Human correction
submission and collector administration have no agent tool.

```sh
hivectl collectors status
hivectl collectors release-uncertain project ITEM_ID --request-id release-1
hivectl collectors rebind project COMB_UUID --request-id rebind-1
hivectl collectors discard project --request-id discard-1
```

Mutation commands require a retained destination shown by `collectors status`,
including destinations removed from the configuration. `release-uncertain` requires
an attempted item at that destination; `rebind` requires a valid UUID. Invalid
selections return exit 2 with `DESTINATION`, `ITEM` or `COMB_ID` diagnostics.

`collectors status` rejects `--request-id`: read-only status has no durable mutation
to retry. The other collector commands accept it before or after their subcommand.

`status` reports counts/bytes, enabled/paused state, pinned identity, gap bounds,
last error, and attempted items with their IDs/generations. `release-uncertain`
accepts possible duplication and lets one attempted item use the current credential
under a new generation; a late acknowledgement cannot remove that replacement.
`rebind` explicitly chooses a Comb UUID and unpauses it; attempted items still need
release or discard. `discard` removes that destination's outbox and gap; an already
submitted external write cannot be undone. All four commands are human-only.

Removing/disabling configuration retains and drains accepted evidence. Each
outbox is capped at 1,024 items/16 MiB; overflow emits one warning per gap episode,
then queues a gap notice when capacity returns. Agent purge and compaction do not
remove accepted evidence. See [Comb collection, curation and recovery](../comb/README.md#evidence-collection-and-curation).
