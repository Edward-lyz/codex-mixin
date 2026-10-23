# macOS automatic updates

The menu app checks release metadata hourly using HTTP HEAD and cached ETag /
Last-Modified validators. This avoids downloading the release HTML body during
routine checks. A newer compatible application release is downloaded by
Sparkle, which retains signature validation and application replacement.

Installation waits until the gateway reports zero active requests and at least
30 minutes since the last request completed. Streaming HTTP bodies, actual
Responses WebSocket requests and realtime connections keep the gateway busy.
Status/model polling does not reset the idle window. Missing activity telemetry,
including an older or stopped gateway, postpones installation.

Each release has at most three automatic attempts with persisted exponential
backoff: the second attempt waits at least one hour, the third another two
hours. A newer target version resets the budget. Non-writable installations
are skipped. Background cycles suppress update UI and do not launch the manual
DMG watchdog or drag-to-Applications fallback.

The standalone gateway survives replacement of the menu app. The automatic
relaunch reuses a running gateway so a request arriving after the idle snapshot
is not interrupted. Its executable changes on disk, but its running code stays
at the old version until a later normal gateway restart. Manual update actions
keep the existing interactive Sparkle/watchdog behavior.

`codex-mixin --no-tui update --check --json` exposes the metadata check without
installing or restarting anything. Policy and HTTP tests cover retry persistence,
idle thresholds, cancellation and conditional redirects. A signed release
installation must be validated separately from a local ad-hoc build.
