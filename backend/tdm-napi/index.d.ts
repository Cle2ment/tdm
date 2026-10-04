/**
 * @typedecision/runtime — native binding surface.
 *
 * Types come from `@typedecision/contract` (ts-rs generated from the Rust
 * core, `backend/tdm-core`): the Rust `#[napi]` exports in
 * `backend/tdm-napi/src/lib.rs` speak exactly this wire shape.
 */
import type { DecisionRequest, DecisionResult, HealthReport } from "@typedecision/contract";

/** Harness + session identity attached to a call (tags its audit/cache rows). */
export interface SessionCtx {
  harness: string;
  sessionId: string;
}

/** Optional per-call overrides. */
export interface JudgeOptions {
  /** Explicit provider override — must name a configured provider (`"jev"` / `"mock"`); unknown names reject with `InvalidArg`. */
  provider?: string;
  /** Harness + session identity tagging the call's audit/cache rows; the object and each field default to `"unknown"`. */
  session?: SessionCtx;
}

/**
 * Judge one batch of questions through the TDM engine (routing, exact-hash
 * cache, retry + circuit breaker, audit). Resolves with the `DecisionResult`
 * JSON, or rejects with an `Error` whose message is prefixed by the TDM error
 * class (`[client]` / `[server]` / `[network]` / `[quality]` /
 * `[unsupported]`). Results served from the exact-hash cache are
 * indistinguishable from fresh ones (`from_cache` is not exposed in M1).
 */
export function judge(req: DecisionRequest, opts?: JudgeOptions): Promise<DecisionResult>;

/**
 * Probe the selected provider's liveness/connectivity. The probed provider is
 * the configured default route (the one `judge` takes without an override).
 */
export function health(): Promise<HealthReport>;
