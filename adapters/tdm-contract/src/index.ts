/**
 * TDM contract: types generated from the Rust core (`backend/tdm-core`) by
 * ts-rs. Never hand-edit `./generated/*`; regenerate via `cargo test` in the
 * backend workspace (D7: Rust types are the single source of truth).
 *
 * NOTE: No build step at M0 — this package ships raw TypeScript source
 * (see `exports` in package.json) and is consumed directly by the workspace
 * TS toolchain. A bundled `.d.ts` build is deferred to M5.
 */

export type { Answer } from "./generated/Answer";
export type { AnswerValue } from "./generated/AnswerValue";
export type { Capabilities } from "./generated/Capabilities";
export type { DecisionRequest } from "./generated/DecisionRequest";
export type { DecisionResult } from "./generated/DecisionResult";
export type { HealthReport } from "./generated/HealthReport";
export type { Primitive } from "./generated/Primitive";
export type { PrimitiveKind } from "./generated/PrimitiveKind";
export type { ProviderMeta } from "./generated/ProviderMeta";
export type { Question } from "./generated/Question";
export type { JsonValue } from "./generated/serde_json/JsonValue";
export type { Usage } from "./generated/Usage";
