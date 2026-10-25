# Build and test policy

Always use release builds for validation, tests, scenarios, and benchmarks. Never
run them against debug builds.

- Build binaries with `cargo build --release --locked`.
- Run Rust tests with `cargo test --release --locked`.
- Run Clippy with `cargo clippy --release --locked --all-targets -- -D warnings`.
- Run CLI and Python scenarios against `target/release/hivectl` (and
  `target/release/hive-daemon` when needed). Build first; do not fall back to debug
  binaries. Explicit binary overrides must also point to release builds.
- `cargo fmt --check` is independent of the build profile.

For installer argument and bootstrap validation, first build the helper with
`cargo build --release --locked -p hive-install`, then run
`python3 -m unittest discover -s tests -p test_installer_arguments.py`.
The suite requires that release binary and uses isolated local transport fixtures.

Keep historical measurements labeled with the build profile actually used.

On a shared physical host, run the macOS validation campaign to completion before
starting native Linux validation in its local container or VM. Do not overlap
their builds, tests or scenarios. Keep `RUST_TEST_THREADS=4` and existing deadlines;
retain failures separately from any focused reruns. This reduces interference but
does not establish a cause or fix for intermittent failures. Independent hosted
runners may still run in parallel.

Compatibility corpora live under `compat/`. The synthetic-0 recipe uses current
formats and deterministic mock providers; it is not a development-format reader.
Its populated roots are generated and upgraded in place so native paths and
inode-bound ownership evidence remain valid. Never rewrite that evidence to load
a copied fixture. Rust corpus and transcript tests run in the normal release suites.
After plain package-scoped Hive and Comb release builds, run the binary matrix:

```sh
python3 scripts/check_compatibility.py --current-dir target/release --output-dir dist/compatibility
```

The output directory must be new. `compat/releases.json` is empty until an explicit
release declaration. Future entries pin each native GitHub release archive and
each consumed executable and corpus recipe by SHA-256. Recipes pin every mock,
input and transcript; released copies live under `compat/<product-version>/`.
The driver upgrades every release's mock-produced corpus, checks newer-data
refusal and runs both control/service/MCP directions for the current and previous
release lines. Older lines get protocol-range checks and explicit refusal when
they have no shared protocol. Add the immutable recipe and fixtures at declaration.
No real provider account is used by this matrix. Live provider canaries require a
separate authorized release environment; ordinary CI only uses recorded fixtures.

After the release contract tests, run
`PYTHONPATH=scripts python3 -m release.compatibility_gate --baseline HEAD` for
local changes (CI compares with its parent commit). The inventory binds each
registry to production-tested fixtures; it is not a source-schema inference tool.
Incomplete corpus coverage is listed as warnings and never blocks an explicit
release declaration from freezing the formats as they stand. Keep those gaps
accurate; closing them is not required for RV7. Already-released fingerprints
remain immutable.

Write, privacy and error-handling inventories live in the internal tools checkout,
not in product CI. Set `HIVE_ROOT` explicitly and run the maintainer checks before
pushing paired product/tools changes:

```sh
export HIVE_ROOT="$PWD"
export HIVE_TOOLS="$HIVE_ROOT/../common_document_storage/hive/hive-tools"
python3 "$HIVE_TOOLS/checks/check_product_policy.py"
```

Install the pinned requirements from `$HIVE_TOOLS/checks/requirements.txt` first.
Each exact error candidate needs its actual type, handling decision and source
evidence; never refresh fingerprints just to silence a gate. For every supported
package/feature variant, run the same driver with `--variant NAME`; its release
Clippy diagnostics feed both exact inventories. See the internal checks README
for the ten names and private CI. The private workflow runs on common_document_storage main
pushes or explicit dispatch for a trusted Hive SHA; Hive-only pushes do not trigger
it. Product builds, tests, packaging and public CI never require that checkout.
No pending package classification is an approved error-handling exception.

The internal refactoring metrics driver checks the declared fourth-pass targets:
`python3 "$HIVE_TOOLS/scripts/refactor_metrics.py" --root "$HIVE_ROOT" --check-targets --json`.
It reports misses and exits nonzero; preserve the raw counts and declared limits.
This is a maintainer check, never a product build or installation dependency.

# Release status and format policy

Hive is unreleased until the user explicitly declares it released. Tags, public
repositories, downloads, installers and completed implementation tasks do not make
that declaration. Never publish a release or freeze a released registry entry as
an implicit consequence of implementing release support.

Maintain one current development shape per owned data format and protocol. The
compatibility registries introduce each at 1; all numbers stay at 1 until the
declaration, and the released-format registry stays empty. Keep exact structural
checks (including schema SQL and strict parsing) in addition to markers. Do not
add development migrations, legacy readers, compatibility aliases, markerless
adoption or fallback bindings. Refuse unsupported development state unchanged,
including missing required markers in a nonempty root. Use a new state directory
or knowledge root; never automatically reset identities, seal keys or credentials.
Preserve current provider protocol handling and corruption/ownership checks.

The declaration freezes that product version's exact formats and protocols.
Afterward, every newer release must migrate data from any earlier released format
through reviewed steps, after a durable backup, and refuse newer formats safely.
A changed frozen format needs a new number, a frozen fixture and a migration;
derived indexes may instead be rebuilt. Numbers change only with the corresponding
format or protocol, never merely for a development iteration. Draft formats may
change freely between releases; development data still has no compatibility
promise. Synthetic migration/skew steps remain test-only. Support current and
previous released protocols with exact per-protocol shapes, without inventing a
predecessor before the first release. Product patch releases do not change format
or protocol numbers. Release support and the eventual release cut are separate.

# Documentation and incubation boundary

Style the product name as **Hive** in prose and display text. Keep executable,
package, path and tool identifiers lowercase (`hive`, `hivectl`, `hive-daemon`, `hive-install`,
`comb`, `hive_*`, `comb_*`). Branding changes must not rewrite stored markers or historical evidence.

The explicitly approved pre-release Hive/Comb rename is an exception for owned
stored names only: current format numbers remain 1, existing user state and roots
are left untouched, and the renamed product uses fresh state and knowledge roots.
Recorded evidence remains byte-identical. This does not authorize legacy readers,
aliases, development migrations or a general change to the rule after release.

Hive contains product code, user documentation, skills, examples and the tests and
drivers needed to validate it. Keep the README, CLI reference, installation and
operating guides complete without access to an internal repository.

New plans, specifications for future work, investigations, review notes and recorded
benchmark/validation results belong in `../common_document_storage/hive/docs/`.
Documentation is maintained separately from product code and internal tools.
Read `../common_document_storage/README.md` when present for its index and evidence rules. Do not recreate an
internal docs archive here or make builds/tests/packages require that checkout.
Generated scratch outputs may stay ignored locally; durable internal write-ups go
there. If the sibling is absent, keep temporary notes outside Hive and report
where they are, rather than adding them to the product docs.

Parallax is permanently an unreleased working/incubation repository. Selected
experimental code or documentation must be explicitly moved into Hive and reviewed
with product tests/docs before it can be released. Releasing selected work does
not make Parallax released. Product manuals and packaged skills must not link to
Parallax; maintainer guidance such as this file may identify the internal workspace.

# Fixture regeneration

Comb fixtures under `comb/tests/fixtures` pin exact serialization, protocol
output and the public error catalogue. Do not regenerate them to hide a regression.
For a deliberate reviewed contract change, run the filtered release test with
`COMB_RECORD_GOLDENS=1`, inspect its diff, then rerun without that variable.
The error-catalogue checker lives under
`../common_document_storage/hive/hive-tools/comb/benchmarks/`; set `HIVE_ROOT` to this checkout.
Product tests read the committed fixtures and never require it.

# Release assembly and acceptance

Use package-scoped commands to avoid workspace feature unification. Do not package
Cargo test-produced binaries. The four part builds must be plain release builds.
Python 3.10+ is needed only for these maintainer tools.

The `barrier-delay` feature is a storage measurement build: `HIVE_BARRIER_DELAY_MS`
(0–1000, read at daemon start) delays each successful ledger barrier. It is a CI
and policy variant (`hive-barrier-delay`), never a part; its version names the
feature, so the build driver and the installer refuse it.

### Assemble a native set from source

Python is required here, on the build machine only. Each output directory must be
fresh. Run builds serially; the driver refuses a source change during a build.

```sh
for part in hive hive-core comb hive-install; do
  python3 scripts/build_release.py --part "$part" --output-dir "dist/frozen/$part"
  python3 scripts/package_release.py --build-dir "dist/frozen/$part" --output-dir dist/assets
done
```

`--clean` on the build driver uses a new temporary Cargo target directory to
measure a clean build. Both modes freeze the plain package build before any tests,
record its hashes and dependency notices, and require that record when packaging.
Packaging refuses non-system libraries and, on Linux, any C++ runtime requirement.

### Check an installed set

The native acceptance driver installs into fresh prefixes, starts deterministic
mock agents, runs a separate installed Comb server, checks collector delivery,
and indexes through a fixture HTTP embedding endpoint before vector search.
It never uses real provider accounts or real models:

```sh
python3 scripts/check_release_install.py --assets dist/assets --output-dir dist/installed-checks
```

The output directory must be new. It retains installed prefixes, logs and a JSON
report with verified plain-release binary hashes. Temporary test state is private
and short-lived. CI runs this check on the native Ubuntu 22.04 and macOS 15 legs.


# Product text and file categories

`scripts/release/policy.py`, exercised by `tests/test_release_packaging.py`,
classifies every tracked file and checks retained guides/examples, package
summaries, CLI help, MCP descriptions/instructions and setup/installer output.
Classify a new product path explicitly before committing it. Keep plans, scale
harnesses, experiments and recorded results in the internal workspace; do not
expand a product category just to admit them. Stage intended deletions before
running the tracked-file check. Local untracked task lists are not product files.

Keep installed-product examples and actionable operating limits in product docs.
Build/test/packaging instructions for maintainers belong here. Only this file may
refer to the internal workspace. Internal tools use explicit `HIVE_ROOT`; product
builds, packages and tests must work without the sibling checkout.

# File publication checks

Use the crates.io `atomic-fs` primitives for file publication and
`scripts/release/files.py` for Python publication. The exact writer inventory is
`$HIVE_TOOLS/quality/write-sites.json`; new or changed write candidates require a
reviewed entry with a conversion stage, reason, frequency and durability decision.
Pending entries are conversion work, not permanent exemptions. The static gate
includes inactive platform code; the private workflow also checks resolved Clippy diagnostics with
`--force-warn clippy::disallowed_methods`, overriding transitional local allows.
Run `PYTHONPATH="$HIVE_TOOLS" python3 -m quality.write_gate` and
`PYTHONPATH="$HIVE_TOOLS" python3 -m unittest quality.test_write_gate`.
Product helper tests remain
`PYTHONPATH=scripts python3 -m unittest release.test_files release.test_publication`.
Never downgrade authoritative durability to satisfy a performance check. Select
`AtomicVisible` only for a justified regenerable output; preserve Comb's ordered
and coalesced barriers through the helper's durability interface.

# Diagnostic logging checks

Emit product diagnostics with qualified `tracing` macros and explicit static event
names. Use only fields in `logging/src/fields.json`: bounded identities, codes,
counts, flags and native `Error` references. Never log raw messages, prompts,
provider payloads, credentials or error `Debug`/`Display`. The `hive-log` backend
filters library fields and owns bounded queues, rotation and recovery. Spans carry
static names, not field payloads. Reserve target `hive.fatal` for fatal `error!`
events; ordinary errors use the normal queue.

Run `PYTHONPATH="$HIVE_TOOLS" python3 -m quality.log_gate` and
`PYTHONPATH="$HIVE_TOOLS" python3 -m unittest quality.test_log_gate`. The gate includes
inactive Rust code; its exact privacy-canary exceptions are not general opt-outs.
Review field semantics: the lint cannot prove a value labeled as an ID is safe.

# Python maintainer diagnostics and error checks

Use `scripts/release/progress.py` for long-running maintainer commands: wrap the
entrypoint with `entry`, group work with `phase` (ten-second heartbeats), and use
`finalizer`/`cleanup_all` so secondary cleanup errors cannot replace the primary
failure or prevent the remaining cleanup. `scripts/release/process.py` checks
subprocess exit status before parsing JSON. A nonzero status is never an empty
successful response. Explicit negative tests still inspect their expected status.

The Python backend uses standard `logging` with a bounded 1,024-record queue,
8 KiB record limit and 250 ms shutdown wait. A daemon writer owns a duplicate of
stderr; a stuck stderr consumer does not block interpreter exit. Causes contain
bounded types, source locations, errno, status and output sizes, never exception
arguments, subprocess output or source text. Library messages are redacted.
Loss counters are available from `Session.finish()`; the final loss receipt is
best effort. If a diagnostic worker cannot start, entry fails before work begins.

Install Ruff 0.17.1 for the mandatory Python error gate, then run:

```sh
PYTHONPATH="$HIVE_TOOLS" python3 -m quality.python_error_gate
PYTHONPATH=scripts python3 -m unittest discover -s scripts/release -t scripts
```

The gate checks BLE001, S110, SIM105, TRY400, PLW1510 and S101. Bare assertions are
allowed only in tests. Deliberate catches have exact source-bound decisions in
`$HIVE_TOOLS/quality/python-errors.json`; changed bodies, new sites and stale decisions
fail. The Rust error inventory likewise permits no pending package classifications.
