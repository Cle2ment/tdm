# TDM — Typed Decision Model

<!-- Status badge placeholder: un-comment once the repo is public. -->
<!-- [![CI](https://github.com/Cle2ment/tdm/actions/workflows/ci.yml/badge.svg)](https://github.com/Cle2ment/tdm/actions/workflows/ci.yml) -->

TDM turns subjective judgment calls — choices, yes/no judgments, ordered scores — into a typed,
auditable API. Instead of free-form "ask the model and hope", callers submit a **typed question**
(one of three primitives: `choice`, `noul`, `score`) with all decision context, and receive a
**typed answer** with a winning label, probability distribution, and confidence.

## Architecture

A **Rust backend** owns everything shared: the contract crate (`tdm-core`: primitives, provider
trait, error taxonomy), provider adapters, and the runtime engine (routing, caching, audit, rate
limiting, circuit breaking), managed by the `tdmm` CLI. **TypeScript harness adapters** integrate
TDM into coding agents (opencode, pi, dsh, MCP). The two worlds meet exactly once: ts-rs generates
TS types from the Rust contract (committed under `adapters/tdm-contract/`), and napi-rs provides
the native binding; adapters consume generated types + client abstraction, never raw Rust.

## Repository layout

```
tdm/
├── backend/                      # Rust workspace (cargo)
│   ├── tdm-core/                 # Contract: types, Primitive, provider trait, errors, exports
│   ├── tdm-runtime/              # Engine: registry, routing, cache, audit (later wave)
│   ├── tdm-provider-*/           # jev / mock / startlux providers (later waves)
│   ├── tdmm/                     # Management CLI, single binary (later wave)
│   └── tdm-napi/                 # napi-rs binding → @typedecision/runtime (later wave)
├── adapters/                     # TypeScript workspace (pnpm)
│   ├── tdm-contract/             # ts-rs generated types + JSON Schemas (committed)
│   ├── tdm-client/               # TdmClient: NapiClient | RpcClient (later wave)
│   └── tdm-{opencode,pi,dsh,mcp}/  # Harness adapters (later waves)
└── docs/adr/                     # Architecture decision records
```

## Development

```sh
cargo test --workspace    # Rust tests; regenerates TS bindings + JSON Schemas
pnpm exec biome check .   # Lint + format TS adapters
```

## Community

- [Contributing](CONTRIBUTING.md) — setup, verification gates, conventions
- [Security](SECURITY.md) — reporting vulnerabilities; key/audit-data handling
- [Code of Conduct](CODE_OF_CONDUCT.md)

## License

Licensed under [Apache-2.0](LICENSE).
