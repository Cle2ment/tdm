# Contributing to TDM

Thanks for your interest in contributing. This document is the single source of
truth for how to build, test, and propose changes. Keep it current: **whenever
the toolchain, commands, or conventions below change, update this file in the
same commit.**

## Development setup

| Tool | Version | Notes |
| --- | --- | --- |
| Rust | stable (pinned by `rust-toolchain.toml`) | Windows: MSVC Build Tools required |
| Node.js | >= 20 (26 used in CI) | type stripping runs raw TS |
| pnpm | 12.9.1 (pinned via `packageManager`) | workspaces: `adapters/*`, `backend/tdm-napi` |
| Bun | latest | runs the napi smoke test (`smoke:bun`) |

```sh
git clone https://github.com/Cle2ment/tdm && cd tdm
cargo test --workspace    # builds the Rust workspace; regenerates TS bindings
pnpm install              # links the TS workspace
```

## Verification gates (all must pass before a PR)

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm -r --if-present typecheck
pnpm -r --if-present test
pnpm exec biome check .
```

CI runs the same gates (see `.github/workflows/ci.yml`), plus the napi smoke
test on node and bun across windows/ubuntu/macos.

## Conventions that will bite you if you skip them

1. **The contract lives in Rust.** Never hand-edit
   `adapters/tdm-contract/src/generated/` or `schema/` — edit
   `backend/tdm-core` types, run `cargo test`, and commit the regenerated
   artifacts in the same commit as the type change.
2. **TypeScript only.** All JS-ecosystem code (packages, scripts, loaders,
   smoke tests) is written in TypeScript; plain JS is only acceptable where a
   runtime mandates it.
3. **New providers must pass the conformance battery**: call
   `tdm_conformance::assert_compliant` from the provider's test suite (see
   `backend/tdm-provider-mock/tests/conformance.rs` for the wiring pattern,
   `backend/tdm-provider-jev/tests/conformance.rs` for the wiremock variant).
4. **New config keys** must stay parse-compatible with the `tdmm init`
   skeleton (`backend/tdmm/src/config.rs`) — missing sections fall back to
   built-in defaults by design. Schema-breaking changes bump `version` in
   `config.toml` and need a migration note.
5. **Keys never leave the auth plane**: `auth.toml` / `TDM_*_API_KEY` env are
   read by the backend only; adapters never see key material. Debug output
   must redact (see `TdmConfig`'s manual `Debug` impl for the pattern).
6. Atomic, conventional-commit-style messages (`feat(scope):`, `fix(scope):`,
   `chore:`, `test:`, `docs:`, `ci:`, `refactor:`).

## Repository layout

See the README. In short: `backend/` is the cargo workspace (contract,
providers, runtime, `tdmm` CLI, napi binding), `adapters/` is the pnpm
workspace (generated contract package, client, per-harness plugins, MCP
server), `docs/adr/` holds decision records, `docs/reviews/` holds milestone
reviews.

## Decision records

Non-trivial architectural choices get an ADR in `docs/adr/` (see ADR-0001…0005
for the format: Status / Context / Decision / Consequences, a page at most).

## License

By contributing you agree your contributions are licensed under the project's
[Apache-2.0 license](LICENSE).
