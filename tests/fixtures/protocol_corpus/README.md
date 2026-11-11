# Protocol conversion corpus

Each case replays one recorded upstream stream through the gateway and compares
the provider request and the client-visible events with `expected.json`.

- `case.json`: client endpoint, upstream protocol and `api_path`, client request.
- `upstream.sse`: the raw stream returned by the mock provider. Streams are
  sanitized, minimal versions of real provider responses (Anthropic Messages,
  OpenAI Chat Completions, Gemini `streamGenerateContent`); ids and usage are synthetic.
- `expected.json`: generated. Ids and timestamps are replaced with `<volatile>`.

Add a case by creating `case.json` and `upstream.sse`, then run

    CODEX_MIXIN_RECORD=1 cargo test --locked --test gateway_http corpus::

and review the new `expected.json` before committing.
