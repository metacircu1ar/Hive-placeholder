# Comb contract fixtures

These fixtures pin authoritative bytes and public output.
Never refresh them merely to make a refactoring test pass.

- `page-*`, `seal-*`, and `*-digests`: every actor encoding, raw accepted UUID
  spelling, rendered Markdown, MAC preimage, seal, promotion and content witnesses.
- `identity`, `population`, `head`, `receipt`, `intent-*`, `service-key`,
  `credential-registry`, `connection`: exact serialization from the durable paths.
- `mutation-receipts`: actual persisted receipts for put, append, capture,
  promotion, decision, and both consolidation writes. A fixed identity and fixed
  seed pages make even CAS inputs deterministic; no fingerprint is redacted.
- `chunks`, `links`, `vectors`, `space-*`, `search-*`: exact chunking, cache keys,
  little-endian vector bytes, engine identities, retrieval output and score bits.
  Vectors come from a deterministic in-process test embedder.
- `socket-*`, `mcp-*`, `hosted-*`: newline framing, optional error fields, session
  handling and tool schemas. Hosted fixtures cover Comb's contribution to the
  combined catalog; unrelated Hive tools retain their own contract tests.
- `cli-*`: actual subprocess stdout and exit codes. Explicit JSON pointers name
  the only replaced values (validated UUIDs, 64-character hashes, temporary paths).
  The original stdout is otherwise compared byte for byte, including whitespace.
- `error-conversions`: representative open-ended IO/SQLite/serde diagnostics.
  `../error-catalog.json` separately inventories literal errors and dynamic
  diagnostic families, which cannot have a finite exhaustive list of messages.

Normal validation never rewrites fixtures. See [maintainer guidance](../../../../AGENTS.md)
for deliberate fixture regeneration.
