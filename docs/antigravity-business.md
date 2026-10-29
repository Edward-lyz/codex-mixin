# Antigravity Business

Use preset `agy-business` for the Business option backed by Gemini Enterprise.
The existing `agy` preset continues to use personal subscription OAuth.

Add the provider with `codex-mixin providers add --preset agy-business`.
Without a usable local Business login, Mixin opens its Google browser OAuth
flow with PKCE and requests the `aicode`, `cloud-platform` and `userinfo.email`
scopes. The native macOS, Windows and Web forms use the same CLI workflow.
Owned credentials are stored privately in `oauth/agy-business.json` beside
the Mixin configuration, independently of personal `oauth/agy.json`.

Mixin can also reuse a Business login from the native `agy` CLI and its
selected Google Cloud project and location. Alternatively, configure authorized-user ADC with
`gcloud auth application-default login --project PROJECT_ID`. Mixin reads the
standard ADC path or `GOOGLE_APPLICATION_CREDENTIALS`; it does not modify native
credentials. The native token must identify its auth method as Business/GCP.

Project selection uses `gcp.project` in the native CLI settings, then ADC's
`quota_project_id`, then `GOOGLE_CLOUD_QUOTA_PROJECT`. Location uses
`gcp.location`, then `GOOGLE_CLOUD_LOCATION`. A single matching provisioned
license is required. Missing or ambiguous licenses return an explicit error.

Discovery and inference carry the license's project, location and `userTier`.
Inference selects the global, US or EU Business endpoint and sends the flat
Gemini payload. It does not send the personal subscription envelope or fall
back to personal discovery. OAuth refresh uses the credential's own client
metadata, and a pre-stream 401 can cause one refresh attempt.

Supported credential forms are Mixin-owned Business OAuth, a valid native Business OAuth token and
`authorized_user` ADC. Service-account, WIF and other ADC types are not yet
implemented and return an explicit error. An expired native token without
refresh client metadata must first be renewed in `agy`, or replaced by a new
Mixin browser login. Owned credentials take precedence; token renewal and
refresh-token rotation are saved atomically without rewriting imported native
or ADC files. A concurrent new login is preserved if an older refresh finishes
after it. Real Business license, model discovery and inference still require
account-level validation; local tests cover authorization dispatch, credential
selection, HTTP refresh, persistence, target validation and wire conversion.

The reference [Pi Business fork](https://github.com/vini1006/pi-antigravity-business/tree/9e73ed6ab7280db2988cb90b0d3b5b3ce9937ccc)
also uses Google OAuth before routing requests to the Business AI Code endpoint.

Official references:

- [Antigravity Enterprise](https://www.antigravity.google/docs/enterprise/)
- [Antigravity CLI setup](https://www.antigravity.google/docs/install?tab=cli)
- [Google Cloud ADC](https://cloud.google.com/docs/authentication/application-default-credentials)
