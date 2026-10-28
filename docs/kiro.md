# Kiro native provider

Sign in through Kiro IDE or `kiro-cli login` on the same machine. Mixin can
discover the session automatically, or add it using
`codex-mixin providers add --preset kiro`. Leave the key empty for local sessions; `ksk_` API keys can also
be configured as provider keys. Mixin does not run a separate Kiro login flow.

Mixin reads the kiro-cli database before the IDE file
`~/.aws/sso/cache/kiro-auth-token.json`. Builder ID / Identity Center, social,
and external IdP CLI sessions are supported. To select an IDE credential file
explicitly, set `CODEX_MIXIN_KIRO_AUTH_FILE`; this disables CLI database lookup.
Expired CLI sessions are refreshed by `kiro-cli debug refresh-auth-token`, so
the installed CLI must be on PATH. IDE desktop and AWS SSO OIDC sessions are
refreshed into Mixin's private OAuth cache without changing the IDE file.
`KIRO_PROFILE_ARN` can select a specific account profile.

Model discovery uses the account's paginated `ListAvailableModels` directory.
There is no bundled model list. A missing session or a failed directory request
returns an error instead of presenting invented models. Native credentials are
accepted only for official `https://runtime.<region>.kiro.dev/` endpoints.
`codex-mixin quota --provider kiro --json` reports credit usage, limits,
remaining credits and reset times, including a separate trial-credit row when
the account reports one. Use the configured provider ID if it differs from
`kiro`. The same quota contract is consumed by the desktop and web interfaces.

Requests use Kiro conversation history, images, tools and tool results. Responses
are decoded incrementally from AWS EventStream with frame size and CRC checks,
then converted through the existing Anthropic/Responses adapters. A truncated
frame, unfinished tool call or upstream exception fails the stream.

This version supports automatic tool selection. Explicit forced tool selection,
thinking and speed settings return an unsupported-feature error. Account model
metadata remains unknown when the directory does not declare a capability.
Authenticated inference still requires validation with a real Kiro account.

References: [pi-kiro-connect](https://github.com/vnlebaoduy/pi-kiro-connect)
for Kiro CLI authentication and management API behavior, and [AWS EventStream
encoding](https://docs.aws.amazon.com/transcribe/latest/dg/event-stream.html).
