# Antigravity Business

Use preset `agy-business` for the Business option backed by Gemini Enterprise.
The existing `agy` preset continues to use personal subscription OAuth.

Sign in to Business with the native `agy` CLI and select its Google Cloud
project and location. Alternatively, configure authorized-user ADC with
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

Supported credential forms are a valid native Business OAuth token and
`authorized_user` ADC. Service-account, WIF and other ADC types are not yet
implemented and return an explicit error. An expired native token without
refresh client metadata must first be renewed in `agy`. Real Business license,
model discovery and inference require an account-level validation; local tests
cover target validation and wire conversion only.

Official references:

- [Antigravity Enterprise](https://www.antigravity.google/docs/enterprise/)
- [Antigravity CLI setup](https://www.antigravity.google/docs/install?tab=cli)
- [Google Cloud ADC](https://cloud.google.com/docs/authentication/application-default-credentials)
