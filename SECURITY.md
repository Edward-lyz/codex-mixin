# Security Policy

## Reporting a vulnerability

Report vulnerabilities privately through GitHub Security Advisories:
<https://github.com/Edward-lyz/codex-mixin/security/advisories/new>.
Please do not open a public issue for security problems.

## Supported versions

Only the latest release receives security fixes.

## Security model

- The gateway listens on loopback (`127.0.0.1:8317`) by default.
- When bound to a non-loopback address, the gateway generates an `sk-` access key on first start.
- Requests from loopback and private-LAN peers need no key; public peers receive 401 unless they send the key in `Authorization: Bearer <key>` or `x-api-key`.
- The web panel requires the token generated at each start.
- Provider credentials are stored in the local configuration file.
