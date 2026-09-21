# Ambient suggestion routing

Codex Desktop starts ambient suggestion generation and suggestion safety checks
as project-level, independent ephemeral turns. They do not inherit a provider
from the visible task or from any other parent task. The
`ambient_suggestions_upstream` setting is the default provider for ambient
requests that arrive without an explicit provider-qualified model or source
provider.

To route this work through a configured provider:

```sh
codex-mixin provider update PROVIDER_ID --ambient-suggestions-upstream true
```

Restart the gateway after changing this setting. Use `provider list --json` to
inspect `ambient_suggestions_upstream`. Set it to `false` to restore the default
model routing.

This is an independent, opt-in setting for ownerless ambient work. It does not
select the auxiliary upstream for voice, image generation, or guardian auto
review. It does not change normal user turns or existing subagents, including
turns using the same model names. Only one provider can own the ambient default
at a time.

When ambient work belongs to a source conversation, callers can pass
`client_metadata["mixin_source_provider"]` with `"official"` or the exact ID of
a configured provider. This optional Mixin field belongs in the request body,
beside `client_metadata["x-codex-turn-metadata"]`, not inside that metadata value
or an HTTP header. It must be present on the initial prewarm request, every
generation or continuation request, and the separate safety-check request.
Callers select it before starting the thread or prewarm; the gateway neither
remembers it across requests nor reads another task's state to discover it.

For example, a generation request using `gpt-5.6-terra` and source provider
`"baidu-oneapi"` runs `gpt-5.6-terra` on Baidu OneAPI. Its safety check can still
use `gpt-5.6-luna` with that same source provider. Source provider `"official"`
keeps those bare model names on the official route. This works even when no
ambient default is configured or a different provider owns that default.

An explicit provider-qualified model slug or fusion slug in the request takes
precedence and is still checked for availability. If a source field is supplied,
its value must be a non-empty string without surrounding whitespace, and its
provider must exist and be enabled. `"official"` requires official routing to
be enabled; the existing official authentication checks still apply. Invalid
source values fail the ambient request instead of falling back to its default.
The gateway consumes the Mixin field before forwarding an ambient request.
Normal user and subagent turns do not use this field.

The inspected Codex Desktop client does not send `mixin_source_provider`.
Its independent project-level ambient requests therefore use the configured
ownerless default. Source-based routing is an explicit caller interface, not
automatic parent-task inheritance. The gateway does not infer a provider from
recent requests, the thread catalog, a parent task, or prompt text.

The gateway identifies the exact `ambient_suggestions` and
`ambient_suggestion_safety` turn triggers in Codex request metadata. For an
ownerless request it maps the requested official model to the same upstream
model name on the configured ambient provider. An explicit source provider
uses the same model mapping on that provider. For a provider-qualified request
it keeps the caller's provider and model unchanged. Both generation and safety
checks still execute; only the selected request route changes. A configured
provider must be enabled and the model must be available and selected. If any
of these checks fails, the request fails instead of falling back to the
official account.

Startup prewarm requests precede the first turn and do not yet carry a turn
trigger. For requests marked `request_kind = "prewarm"` with no turn trigger, the
gateway uses the same two exact values from `thread_source` only to identify the
ambient request kind; it never uses `thread_source` to select a provider. The
provider WebSocket path handles the no-generation warmup locally. Other
requests never inherit routing from an old thread source.

The canonical per-request
`client_metadata["x-codex-turn-metadata"]` takes precedence over the corresponding
HTTP or WebSocket upgrade header, which may belong to an earlier turn on a
reused connection. The gateway also accepts the flat client metadata compatibility
projection. It never inspects prompt text to infer a request's purpose.

HTTP and WebSocket Responses requests use the same policy, including WebSocket
continuations. Provider-qualified model choices remain explicit. Routing logs
include the trigger, thread identifier, requested model, and selected provider;
prompt bodies and credentials are not logged.
