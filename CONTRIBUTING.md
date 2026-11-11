# Contributing

Codex Mixin is licensed under CC BY-NC 4.0 (non-commercial); contributions are accepted under the same license.

## Build

```sh
cargo build
```

## Required checks

CI (`.github/workflows/ci.yml`) runs these on every pull request:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- \
  -D clippy::cognitive_complexity \
  -D clippy::redundant_closure_for_method_calls \
  -D clippy::redundant_pattern_matching \
  -D warnings
./scripts/check_arch.sh
cargo test --locked --tests
./scripts/e2e_multi_provider.sh   # Linux
```

Tests and `scripts/e2e_multi_provider.sh` need `rg` (ripgrep) and `jq`. CI also runs `cargo deny check advisories bans sources`, and clippy plus `./scripts/test_client_auth.sh` with `--features client-auth`.

## Layering rules

See [AGENTS.md](AGENTS.md) for module layering and coding rules; `scripts/check_arch.sh` enforces the boundaries.

Contributors do not need the `codex/` branch prefix.
