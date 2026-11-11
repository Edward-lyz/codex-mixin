# CLI JSON output schemas

JSON Schema (draft 2020-12) for the stdout of `codex-mixin <command> --json`. Native shells and scripts can validate against these files. Objects may gain fields in later versions; consumers should ignore unknown fields. `codex-mixin interface --json` reports `protocol_version`, which changes only on breaking changes.

| Command | Schema |
|---|---|
| `codex-mixin agent list --json` | [agent-list.schema.json](agent-list.schema.json) |
| `codex-mixin agent versions --json` | [agent-versions.schema.json](agent-versions.schema.json) |
| `codex-mixin codex-status --json` | [codex-status.schema.json](codex-status.schema.json) |
| `codex-mixin config keys list --json` | [config-keys-list.schema.json](config-keys-list.schema.json) |
| `codex-mixin config --scope effective --json` | [config.schema.json](config.schema.json) |
| `codex-mixin context config get --json` | [context-config-get.schema.json](context-config-get.schema.json) |
| `codex-mixin context stats --json` | [context-stats.schema.json](context-stats.schema.json) |
| `codex-mixin doctor --quick --json` | [doctor.schema.json](doctor.schema.json) |
| `codex-mixin ech status --json` | [ech-status.schema.json](ech-status.schema.json) |
| `codex-mixin fusion models --json` | [fusion-models.schema.json](fusion-models.schema.json) |
| `codex-mixin interface --json` | [interface.schema.json](interface.schema.json) |
| `codex-mixin providers list --json` | [providers-list.schema.json](providers-list.schema.json) |
| `codex-mixin quota --json` | [quota.schema.json](quota.schema.json) |
| `codex-mixin route list --json` | [route-list.schema.json](route-list.schema.json) |
| `codex-mixin route models --json` | [route-models.schema.json](route-models.schema.json) |
| `codex-mixin status --json` | [status.schema.json](status.schema.json) |
| `codex-mixin usage --json` | [usage.schema.json](usage.schema.json) |
