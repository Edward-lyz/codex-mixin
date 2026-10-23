# Kiro native provider

Add the `kiro` preset after signing in to Kiro on the same machine. Leave the
API key empty to use `~/.aws/sso/cache/kiro-auth-token.json`, or set
`CODEX_MIXIN_KIRO_AUTH_FILE` to another native credential file. Desktop and
registered AWS SSO OIDC sessions can refresh their tokens. Refreshed credentials
are saved only in Mixin's private OAuth cache; the original file is unchanged.

Model discovery uses the account's paginated `ListAvailableModels` directory.
There is no bundled model list. A missing session or a failed directory request
returns an error instead of presenting invented models. Native credentials are
accepted only for official `https://runtime.<region>.kiro.dev/` endpoints.

Requests use Kiro conversation history, images, tools and tool results. Responses
are decoded incrementally from AWS EventStream with frame size and CRC checks,
then converted through the existing Anthropic/Responses adapters. A truncated
frame, unfinished tool call or upstream exception fails the stream.

This version supports automatic tool selection. Explicit forced tool selection,
thinking and speed settings return an unsupported-feature error. Account model
metadata remains unknown when the directory does not declare a capability.
Authenticated inference still requires validation with a real Kiro account.

Protocol reference: [AWS EventStream encoding](https://docs.aws.amazon.com/transcribe/latest/dg/event-stream.html).
