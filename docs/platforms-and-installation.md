# Platforms and installation

## Supported platforms

| Platform | Support |
| --- | --- |
| macOS, Apple Silicon (arm64) | Supported; macOS 11 or newer. Releases are validated on macOS 26. |
| Linux x86_64 | Source builds only; prebuilt archives await native validation. |
| Linux arm64 | Supported; glibc 2.35 or newer. |
| macOS, Intel (x86_64) | Source builds only. |
| Windows | Use the Linux build inside WSL 2. Native Windows is not supported. |

Hive uses Unix sockets, file locks, private permissions and process groups.
Keep state and working directories on a local filesystem that supports them.

### Windows through WSL 2

Install Ubuntu inside WSL 2, then follow the Linux instructions there. Install
Linux Codex and Claude CLIs inside WSL; Windows provider executables cannot be
used. Keep state, knowledge roots and agent working directories on the WSL Linux
filesystem, such as under `~`, rather than a mounted Windows drive under `/mnt/c`.
For semantic search, run an embedding server inside WSL (with NVIDIA CUDA if
available) or use a separate HTTP endpoint.

### Containers

**Run Hive with an init process that reaps orphans:** `docker run --init`,
`init: true` in Compose, or tini.

The daemon adopts and reaps the processes its agents leave behind. But if the
daemon itself is killed, its remaining processes pass to the container's PID 1.
A PID 1 that never reaps them leaves them as zombies, so after a restart Hive
cannot confirm their cleanup, and status keeps showing their work as unsettled.

## Requirements

- Native Codex or Claude Code. Setup can install a missing CLI with explicit
  consent. Provider CLIs must meet the [minimum supported versions](compatibility.md);
  newer admitted versions can be used and are reported as `untested`.
  Hive keeps isolated profiles and does not reuse an ordinary provider login.
- The archive installer needs POSIX `sh`, `curl` for HTTPS sources, and `shasum`
  or `sha256sum`. Installed programs do not require Python.
- Comb text search needs no model. Semantic search uses an HTTP embedding
  endpoint, which runs separately; see [embeddings](../comb/README.md#embeddings).

## Build from source

Install Rust through `rustup` and a C compiler: Xcode Command Line Tools on macOS,
or `build-essential` on Debian/Ubuntu. The pinned Rust toolchain is installed
automatically. TLS uses rustls; OpenSSL is unnecessary.

```sh
git clone https://github.com/metacircu1ar/hive.git hive
cd hive
cargo install --locked --path . --root "$HOME/.local"
# Optional standalone knowledge service:
cargo install --locked --path comb --root "$HOME/.local"
export PATH="$HOME/.local/bin:$PATH"
hivectl setup --skills-dir "$PWD/skills"
```

Cargo installs optimized binaries by default. Add `--no-default-features` to the
Hive install command for core. Continue with the [setup guide](setup.md) for
sign-in, models, services and a first agent.

Keep this directory: Cargo installs executables but does not copy the bundled
skills and manuals. `--skills-dir` points setup to those files; generated role
configuration keeps their paths. Pass the same option when rerunning source setup.

### Command-name clashes

Hive's CLI is named `hivectl` because Apache Hive uses `hive`. The daemon is
`hive-daemon`; no `hive` command or alias is installed. Users may add
`alias hive=hivectl` themselves if desired.

Check which executable your shell selects with `command -v hivectl` or
`command -v comb`. The installer refuses to replace another project's commands;
choose a different prefix to keep both, or use the desired executable's absolute
path.

## Incompatible state

Current builds have pre-release status; see the
[compatibility policy](compatibility.md). Tags and downloadable archives do not
establish a supported release or a data-upgrade promise.

If Hive reports an incompatible state directory, use a new `--state-dir`.
Keep the old directory until you deliberately choose to remove it.

1. Stop the old daemon with its original executable and configuration/state paths.
   Keep that executable if you still need to inspect the old state.
2. Select the new directory for setup, for example
   `hivectl --state-dir "$HOME/.local/state/hive-new" setup`.
3. Use the same configuration and new state directory for every daemon and client
   command. Sign in to the new profiles during setup.
4. If Comb reports an incompatible root, create a new one with
   `comb --root "$HOME/Comb-new" setup`. Preserve old Markdown separately and
   re-add content with current Comb commands. Keep the entire old root, including
   `.comb`; re-adding pages does not preserve its seal key, trust or receipts.
   Issue new bridge credentials.

## Per-part archives

| Archive | Contents |
| --- | --- |
| `hive-<target>.tar.gz` | Comb-capable `hivectl` and `hive-daemon`, CLI reference and agent skill |
| `hive-core-<target>.tar.gz` | Core `hivectl` and `hive-daemon`, documentation and skill |
| `comb-<target>.tar.gz` | Text search and HTTP embeddings |

Each includes third-party notices and a payload manifest. The archive set also
contains `SHA256SUMS`, `install.sh` and `hive-install-<target>` with its notices.
Provider CLIs and embedding servers are separate installations. Linux binaries
require glibc 2.35 or newer; the installer checks staged programs before changing
the installation and reports missing libraries.

### Install a release

The installer downloads the selected archives, its helper and the checksums from
the release, and verifies them before changing anything:

```sh
HIVE_RELEASE=https://github.com/metacircu1ar/hive/releases/download/v0.1.0-preview.1
curl -fsSLO "$HIVE_RELEASE/install.sh"
sh install.sh --from "$HIVE_RELEASE" --prefix "$HOME/.local"
# Add standalone Comb:
sh install.sh --from "$HIVE_RELEASE" --prefix "$HOME/.local" --comb
# Core Hive and standalone Comb:
sh install.sh --from "$HIVE_RELEASE" --prefix "$HOME/.local" --core --comb
export PATH="$HOME/.local/bin:$PATH"
hivectl --version
comb --version
```

These commands pin the current preview. For another published version, change
`download/v0.1.0-preview.1` to its tag. GitHub’s `latest/download` endpoint
excludes previews; it becomes available when a full release is published. `--from` also accepts a local directory holding a release set:
`install.sh`, the archives, `hive-install-<target>` and `SHA256SUMS`. Omitting
`--from` uses the directory containing `install.sh`; a piped script requires an
explicit source. HTTPS redirects remain HTTPS. Checksums detect
inconsistent downloads; obtain both assets and checksums from a trusted source.
Prefix and source arguments are checked before downloads. An absolute prefix is
required; pass `--prefix` explicitly if `HOME` is unset. Invalid options exit with
status 2 and print JSON plus human guidance. Asset loading failures name the
affected file and exit with status 1.
The [setup guide](setup.md) covers every installer and wizard flag.

### macOS downloads and Gatekeeper

Release programs are not notarized. macOS checks only files that carry the
`com.apple.quarantine` attribute, which browsers and some other apps add to
downloads; `curl` does not.

- **Downloaded with `curl`, as above:** no file is quarantined, so the installer
  and the installed programs run without a Gatekeeper prompt.
- **Downloaded in a web browser:** every file is quarantined, and macOS blocks
  the installer helper, `hive-install-<target>`. Remove the attribute from the
  download directory, then install from it:

```sh
xattr -dr com.apple.quarantine "$HOME/Downloads/hive-release"
sh "$HOME/Downloads/hive-release/install.sh" --from "$HOME/Downloads/hive-release"
```

- **Unpacked by hand:** the installer writes every installed program as a new
  file, so installed programs never carry the attribute. If you instead unpack a
  quarantined archive with `tar`, macOS copies the attribute onto each extracted
  file; run the same `xattr -dr` on the extracted directory, or install with
  `install.sh`.

### Ownership and upgrades

The installer verifies selected archives and their payloads before changing the
prefix, then runs staged executables. If the temporary filesystem is mounted
`noexec`, set `TMPDIR` to a private directory on an executable filesystem.
It owns only its links under `PREFIX/bin`, `PREFIX/share/{hive,comb}` and the
manager under `PREFIX/lib/hive`. Unrelated files, altered owned payloads, unsafe
links and incomplete ownership evidence are refused.

The CLI and daemon switch together. Upgrading Hive alone retains the installed
Comb; `--comb` upgrades it too. Binary installation preserves configuration,
state and logins. The following setup wizard applies only the choices it displays; skip
it with `--no-setup`.

Old program images stay available to running processes. The installer reports
their count and distinct disk bytes. It stages before stopping owned services,
restarts only those previously loaded, and keeps stopped services stopped. Retry
the same installer command after an interruption. Manually started processes
require manual control.

Plain `--uninstall` removes selected commands and owned services while keeping
data and old images. `--purge` adds exact data cleanup; whole knowledge roots
require `--purge-comb-root`. See [upgrade and removal](setup.md#upgrade-and-uninstall)
for retained history, retries and running-process checks.

### Installation diagnostics

Installation progress and failures use bounded, privacy-filtered diagnostics on
stderr. Once the prefix is admitted and its installation lock is held, the helper
also records diagnostics in `PREFIX/lib/hive-install-logs/install.jsonl`: at most
five structured files of 10 MiB each, including the active file. This private
log directory is separate from `PREFIX/lib/hive` and remains after image purge.
Uninstall and purge report their progress on stderr. Invalid arguments and dry-run
commands do not create installation logs.

A failed rollback or cleanup reports both the original failure and the cleanup
failure. Keep the recovery files and retry the same command; an interrupted
installation is never reported as successful merely because cleanup succeeded.
If process-image inspection is unavailable, retained-image usage is reported as
unknown rather than zero. Bootstrap progress reports fetch, verification and
helper phases without recording the source URL or credentials.
