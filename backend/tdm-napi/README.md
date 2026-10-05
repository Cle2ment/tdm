# @typedecision/runtime

Native (napi-rs) binding for the TDM decision runtime: `judge` / `health`
run **in-process** through a prebuilt Rust binary — registry, capability
check, exact-hash cache, retry/circuit breaker, and audit are all inside the
`.node` binding, with no HTTP hop.

This package is the native umbrella published to npm as
[`@typedecision/runtime`](https://www.npmjs.com/package/@typedecision/runtime). Per-platform
binaries are shipped as optional-dependency packages (`@typedecision/runtime-<triple>`)
and resolved automatically at import time; a local binding built from a tdm
checkout is used as a fallback during development.

## Install

You normally don't install this directly — it is a dependency of
[@typedecision/opencode-tdm](https://github.com/Cle2ment/tdm) (the OpenCode plugin).
Direct use:

```bash
npm install @typedecision/runtime
```

```ts
import { judge, health } from "@typedecision/runtime";

const result = await judge(
  {
    state: { repo: "my-repo", filesChanged: 3 },
    questions: [
      { id: "q1", instructions: "pick one", primitive: { type: "choice", options: ["a", "b"] } },
    ],
  },
  { provider: "mock" },
);
```

The request/response shapes are defined by `@typedecision/contract`
(`DecisionRequest`, `DecisionResult`, `HealthReport`).

## Requirements

The TypeScript entry (`index.ts`) is shipped as raw TypeScript and executed
directly by the host — no build step, no generated JS loader. That requires a
runtime with **type stripping**:

- **bun** (any recent version), or
- **node >= 22.6** (with type stripping enabled, default in node 23+ /
  `--experimental-strip-types` on 22.6).

Older Node versions are not supported.

## Platform matrix

| Package | OS | CPU | libc |
| --- | --- | --- | --- |
| `@typedecision/runtime-win32-x64-msvc` | Windows | x64 | MSVC |
| `@typedecision/runtime-linux-x64-gnu` | Linux | x64 | glibc |
| `@typedecision/runtime-linux-arm64-gnu` | Linux | arm64 | glibc |
| `@typedecision/runtime-darwin-x64` | macOS | x64 | — |
| `@typedecision/runtime-darwin-arm64` | macOS | arm64 | — |

musl (Alpine) and Windows arm64 are **not** part of the release matrix. The
matching platform package is installed automatically as an
`optionalDependencies` entry; the loader picks it at runtime.

## Local build

From a tdm checkout:

```bash
pnpm install
pnpm --filter @typedecision/runtime build:debug   # napi build --platform --no-js
pnpm --filter @typedecision/runtime smoke         # node smoke test (mock provider)
pnpm --filter @typedecision/runtime smoke:bun     # bun smoke test
```

`build:debug` produces `tdm-runtime.<host-triple>.node` beside `index.ts`,
which the loader picks up when no platform package is installed. `napi build
--no-js` is load-bearing: it keeps the CLI from overwriting the hand-written
`index.ts` with a generated JS loader.

Releases are cut by tagging `v*`, which triggers
`.github/workflows/release.yml`: a 5-target matrix build, then
trusted-publishing (OIDC) `pnpm publish` of each platform package followed by
the umbrella.

## License

Apache-2.0 — see [LICENSE](./LICENSE).
