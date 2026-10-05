/**
 * M2 risk gate: wires TDM risk scoring into the host's tool-execution flow.
 *
 * Verified host mechanics (opencode v2.0.22):
 *
 * 1. `ctx.tool.hook("execute.before", cb)` fires for every tool call with
 *    `{ tool, sessionID, agent, messageID, id, input }`. The callback returns
 *    void — there is NO return-value veto. Enforcement instead goes through:
 * 2. `ctx.permission.hook("evaluate", cb)` — every built-in tool calls
 *    `permission.assert` with `source: { type: "tool", messageID, id }` at the
 *    start of its execute; the host computes a base effect from the user's
 *    permission rules, runs the evaluate hooks, and uses the post-hook
 *    `effect`: `"ask"` prompts the user (rejecting blocks the call),
 *    `"deny"` fails it immediately. `effect` is mutable in the payload; a
 *    hard-deny rule short-circuits BEFORE the trigger, so hooks can never
 *    downgrade a deny.
 *
 * Flow: `onExecuteBefore` stashes a fire-and-forget judgment promise keyed by
 * the call id (skipping allowlisted read-only tools and `tdm_judge` itself);
 * `onPermissionEvaluate` awaits the stashed verdict for the matching call and
 * mutates `effect` to `"ask"` when the policy escalates. Fail-open everywhere:
 * a judge outage or an uncorrelated permission event leaves the effect
 * untouched, so the gate can never break the host's tool execution.
 */

import type { DecisionResult, JudgeOptions, TdmClient } from "@typedecision/client";

import {
  buildRiskRequest,
  evaluateRisk,
  needsJudgment,
  parsePolicy,
  type RiskOutcome,
  type RiskPolicyOptions,
} from "./policy";

/**
 * Structural slice of the host's `execute.before` payload
 * (`@opencode/plugin` promise/tool.d.ts `ToolHooks["execute.before"]`);
 * the SDK's payload satisfies this, tests pass bare objects.
 */
export interface ExecuteBeforeEvent {
  tool: string;
  sessionID?: string;
  id?: string;
  input?: unknown;
}

/** Permission.Effect, from @opencode/schema/permission. */
export type PermissionEffect = "allow" | "deny" | "ask";

/**
 * Structural slice of the host's `permission.evaluate` payload
 * (`PermissionEvaluation`); `effect` and `message` are the mutable fields the
 * host reads back after hooks run.
 */
export interface PermissionEvaluateEvent {
  effect: PermissionEffect;
  message?: string;
  source?: { type?: string; id?: string };
}

export interface RiskGateStats {
  judged: number;
  escalated: number;
  failed: number;
}

export interface RiskGate {
  onExecuteBefore: (event: ExecuteBeforeEvent) => Promise<void>;
  onPermissionEvaluate: (event: PermissionEvaluateEvent) => Promise<void>;
  stats: () => RiskGateStats;
}

/** Bound on stashed judgments; protects against calls that never evaluate. */
const PENDING_CAP = 512;

/** Plugin logging convention: a single `[tdm]`-prefixed console.warn channel. */
function warn(message: string): void {
  console.warn(`[tdm] ${message}`);
}

/**
 * Build the two-hook risk gate against an injected client. Pure — tests pass
 * a fake {@link TdmClient}; the plugin passes the lazily created real one.
 */
export function createRiskGate(client: TdmClient, options?: RiskPolicyOptions): RiskGate {
  const policy = parsePolicy(options);
  const pending = new Map<string, Promise<RiskOutcome>>();
  const stats: RiskGateStats = { judged: 0, escalated: 0, failed: 0 };

  /** Judge one tool call; never rejects (failures degrade to allow + warn). */
  function judge(event: ExecuteBeforeEvent): Promise<RiskOutcome> {
    const request = buildRiskRequest(event.tool, event.input);
    const session: JudgeOptions["session"] = {
      harness: "opencode",
      sessionId: event.sessionID ?? "unknown",
    };
    return client.judge(request, { session }).then(
      (result: DecisionResult) => {
        stats.judged++;
        return evaluateRisk(result, policy);
      },
      (cause: unknown) => {
        stats.failed++;
        const message = cause instanceof Error ? cause.message : String(cause);
        warn(`risk judgment failed; allowing tool call "${event.tool}": ${message}`);
        return { action: "allow", reason: `TDM judge unavailable: ${message}` };
      },
    );
  }

  return {
    onExecuteBefore: async (event) => {
      if (!event || !needsJudgment(event.tool)) return;
      const id = event.id != null ? String(event.id) : undefined;
      if (id === undefined) return; // cannot correlate with a permission evaluation
      const outcome = judge(event);
      if (pending.size >= PENDING_CAP) {
        const oldest = pending.keys().next();
        if (oldest.done !== true) pending.delete(oldest.value);
      }
      pending.set(id, outcome);
    },

    onPermissionEvaluate: async (event) => {
      // Only escalate an allow: never downgrade an existing ask, never touch
      // a deny (hard denies never reach hooks anyway).
      if (event.effect !== "allow") return;
      const source = event.source;
      const id = source?.type === "tool" && source.id != null ? String(source.id) : undefined;
      if (id === undefined) return;
      const outcome = pending.get(id);
      if (!outcome) return; // uncorrelated event → pass-through
      pending.delete(id);
      const verdict = await outcome;
      if (verdict.action === "escalate") {
        stats.escalated++;
        event.effect = "ask";
        event.message = verdict.reason;
      }
    },

    stats: () => ({ ...stats }),
  };
}
