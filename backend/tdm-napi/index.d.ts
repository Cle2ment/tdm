/**
 * @typedecision/runtime — native binding surface.
 *
 * Types come from `@typedecision/contract` (ts-rs generated from the Rust
 * core, `backend/tdm-core`): the Rust `#[napi]` exports in
 * `backend/tdm-napi/src/lib.rs` speak exactly this wire shape.
 */
import type { DecisionRequest, DecisionResult, HealthReport } from "@typedecision/contract";

/** Harness + session identity attached to a call (reserved for M1 runtime wiring). */
export interface SessionCtx {
  harness: string;
  sessionId: string;
}

/** Optional per-call overrides. */
export interface JudgeOptions {
  /** Explicit provider selection: `"jev"` or `"mock"`. */
  provider?: string;
  /** Accepted but ignored until M1 (runtime/session wiring). */
  session?: SessionCtx;
}

/**
 * Judge one batch of questions. Resolves with the `DecisionResult` JSON, or
 * rejects with an `Error` whose message is prefixed by the TDM error class
 * (`[client]` / `[server]` / `[network]` / `[quality]` / `[unsupported]`).
 */
export function judge(req: DecisionRequest, opts?: JudgeOptions): Promise<DecisionResult>;

/** Probe the selected provider's liveness/connectivity. */
export function health(): Promise<HealthReport>;
