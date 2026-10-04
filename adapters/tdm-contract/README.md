# @typedecision/contract

TDM contract types generated from the Rust core (`backend/tdm-core`) by
[ts-rs](https://github.com/Aleph-Alpha/ts-rs), plus committed JSON Schema
copies in `schema/`. Do not edit `src/generated/**` by hand — regenerate via
`cargo test` in the backend workspace.

No build step at M0: the package ships raw TypeScript source (see `exports`)
for workspace consumption; a bundled `.d.ts` build is deferred to M5.
