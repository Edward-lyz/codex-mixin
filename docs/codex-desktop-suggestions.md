# Codex Desktop chat suggestions

Official-account mode uses the built-in `openai` provider identity and routes
inference through Mixin with the supported user-level `openai_base_url` override:

```toml
model_provider = "openai"
openai_base_url = "http://127.0.0.1:8787/v1"
model_catalog_json = "/path/to/model-catalogs/mixin-models.json"
```

The generated config also retains an inactive `[model_providers.codex-mixin]`
table. It records Mixin ownership and managed metadata so detection, catalog
refresh, key rotation and uninstall still work after Codex removes comments.
It does not replace the built-in provider. User-owned provider tables remain
unchanged, and uninstall restores the pre-install config, including an existing
`openai_base_url` value.

Codex Desktop's suggestion generation rejects a resumed thread whose
`modelProvider` is not `openai` with `unsupported_provider`. A custom provider
with `requires_openai_auth = true` does not satisfy this check. The supported
endpoint override keeps the required identity while requests still use Mixin.
See the [official configuration reference](https://developers.openai.com/codex/config-reference/).

The same Rust install and startup code serves macOS, Windows and Linux. Starting
the updated gateway migrates managed official-account configurations that used
`codex-mixin` with WebSocket support, and keeps both endpoint fields synchronized
when the gateway port changes. Reconnecting Codex in official-account mode also
writes the new format. Restart Codex after applying it. Existing session files
and databases are not rewritten; use a new chat if an old thread still reports
the previous provider identity.

The built-in OpenAI provider ignores custom headers in `model_providers.openai`.
For `client-auth` builds, DUCX therefore also accepts the configured official
OAuth token from a verified loopback peer. Remote OAuth, incorrect tokens and
forged loopback headers remain rejected. Custom-only mode keeps its existing
provider and login placeholder; it has no official-account suggestion guarantee.

This fixes the provider eligibility check. Suggestions still depend on Codex's
model eligibility, feature availability and the selected model's response.
