# Codex thread title routing

In a configured Provider's advanced options, enable **Codex 重命名模型映射**
below the auto review model selector, then choose one of that Provider's selected
models. This setting works independently of the auxiliary upstream switch.
Saving a mapping clears the mapping on other Providers. Disable the switch to
restore normal title routing. A disabled Provider does not route titles.

Equivalent CLI commands, from the repository's built CLI or an installed build:

```sh
codex-mixin --no-tui provider update PROVIDER_ID --codex-thread-title-model MODEL_ID
codex-mixin --no-tui provider update PROVIDER_ID --clear-codex-thread-title-model
```

The configuration field is `codex_thread_title_model`; omitted means off.
Removing the selected model also clears the mapping. Duplicate configured
mappings are rejected when loading a hand-edited configuration.

The gateway reads Codex's `thread_source` and `turn_trigger` markers from the
JSON string at `client_metadata["x-codex-turn-metadata"]`, or the corresponding
HTTP/WebSocket header. The body metadata takes precedence. Only an explicit
`thread_title` marker activates the mapping. A model name such as `gpt-6-luna`
or a title-like user prompt alone does not activate it.

HTTP and Responses WebSocket requests reuse the existing provider transports,
authentication and response conversion. Client-facing model names stay intact.
If an explicit mapping cannot route to its selected model, the gateway fails
the request instead of spending official inference quota. The selected Provider
can still charge for its own inference.

The mapping applies to title requests that reach this gateway. An older client
that omits the metadata or a client request sent directly to OpenAI cannot be
identified or redirected here. Tests use local official/provider endpoints to
verify both transports, zero official requests for a mapped title, and unchanged
ordinary-model routing.

Wire reference: [Codex response metadata](https://github.com/openai/codex/blob/main/codex-rs/core/src/responses_metadata.rs).
