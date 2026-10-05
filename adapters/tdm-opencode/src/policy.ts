/**
 * M2 risk policy: pure decision logic for the `execute.before` risk gate.
 *
 * The gate scores every judgment-worthy tool call through TDM with two
 * questions — how risky is executing without asking, and should the agent ask
 * first — then maps the verdict onto an enforcement outcome:
 *
 * - score level "dangerous" → escalate regardless of confidence;
 * - score confidence below the floor (default 0.55) → escalate;
 * - noul P(ask-first) >= 0.8 → escalate;
 * - anything else → allow (pass-through, no user friction).
 *
 * Escalation is enforced by mutating `effect` to `"ask"` in the host's
 * `permission.evaluate` hook (see gate.ts), which makes opencode prompt the
 * user; rejecting that prompt blocks the tool call.
 */

import type { DecisionRequest, DecisionResult } from "@typedecision/client";

/** Default minimum score-answer confidence below which a call is escalated. */
export const DEFAULT_CONFIDENCE_FLOOR = 0.55;

/** P(yes) on the ask-first noul question at or above which a call is escalated. */
export const NOUL_CONFIRM_THRESHOLD = 0.8;

/**
 * Built-in read-only tools judged too harmless to spend a judgment on. V2
 * tool names are lowercase; matching is case-insensitive anyway. Anything
 * NOT on this list (bash, edit, write, MCP tools, unknown tools) is judged.
 */
export const READ_ONLY_TOOLS: ReadonlySet<string> = new Set([
  "read",
  "grep",
  "glob",
  "list",
  "ls",
  "skill",
  "webfetch",
  "websearch",
]);

/** The plugin's own tool — exempt so the judge never judges itself (loops). */
export const SELF_TOOL = "tdm_judge";

/** True when a tool call must go through TDM risk scoring. */
export function needsJudgment(tool: string): boolean {
  const name = tool.toLowerCase();
  return name !== SELF_TOOL && !READ_ONLY_TOOLS.has(name);
}

/** ~2KB cap on the serialized tool args embedded in the judgment state. */
export const ARGS_LIMIT = 2048;

/**
 * JSON-stringify tool args for the judgment state, hard-truncated to ~2KB.
 * Unserializable args (cyclic structures) degrade to a marker instead of
 * throwing — the gate must never crash the host.
 */
export function truncateArgs(args: unknown): string {
  let text: string;
  try {
    text = JSON.stringify(args) ?? "undefined";
  } catch {
    return "[unserializable tool args]";
  }
  if (text.length <= ARGS_LIMIT) return text;
  return `${text.slice(0, ARGS_LIMIT)}...[truncated, ${text.length} chars total]`;
}

/** The DecisionRequest sent to the judge for one tool call. */
export function buildRiskRequest(tool: string, args: unknown): DecisionRequest {
  return {
    state: { tool, args: truncateArgs(args) },
    questions: [
      {
        id: "risk",
        instructions: "How risky is executing this tool call without asking the user?",
        primitive: { type: "score", levels: ["safe", "minor risk", "risky", "dangerous"] },
      },
      {
        id: "confirm",
        instructions: "Should the agent ask the user for confirmation before executing?",
        primitive: { type: "noul" },
      },
    ],
  };
}

/** Plugin options (opencode V2 `{ package, options }` config entry). */
export interface RiskPolicyOptions {
  /** Minimum score-answer confidence; below → escalate. Default 0.55. */
  confidenceFloor?: number;
}

/** Validated policy input. */
export interface RiskPolicy {
  readonly confidenceFloor: number;
}

/**
 * Parse the plugin options defensively: a bad `confidenceFloor` must never
 * break plugin load — warn and fall back to the default.
 */
export function parsePolicy(options: RiskPolicyOptions | undefined): RiskPolicy {
  const raw = options?.confidenceFloor;
  if (typeof raw === "number" && Number.isFinite(raw) && raw >= 0 && raw <= 1) {
    return { confidenceFloor: raw };
  }
  if (raw !== undefined) {
    console.warn(
      `[tdm] ignoring invalid confidenceFloor ${JSON.stringify(raw)}; using ${DEFAULT_CONFIDENCE_FLOOR}`,
    );
  }
  return { confidenceFloor: DEFAULT_CONFIDENCE_FLOOR };
}

/** Enforcement outcome for one judged tool call. */
export type RiskOutcome =
  | { action: "allow"; reason: string }
  | { action: "escalate"; reason: string };

/**
 * Map a judge result onto the enforcement outcome. Pure and total: order of
 * checks is dangerous → confidence → noul, matching the M2 spec. A verdict
 * the policy cannot read (missing/untyped answers) falls through to allow —
 * same fail-open philosophy as a judge outage.
 */
export function evaluateRisk(result: DecisionResult, policy: RiskPolicy): RiskOutcome {
  const risk = result.answers.find((answer) => answer.id === "risk");
  const confirm = result.answers.find((answer) => answer.id === "confirm");

  if (risk?.value.type === "score") {
    if (risk.value.level === "dangerous") {
      return {
        action: "escalate",
        reason: `TDM scored this tool call "dangerous" (confidence ${risk.confidence ?? "n/a"})`,
      };
    }
    if (typeof risk.confidence === "number" && risk.confidence < policy.confidenceFloor) {
      return {
        action: "escalate",
        reason: `TDM confidence ${risk.confidence} is below the floor ${policy.confidenceFloor} (risk level "${risk.value.level}")`,
      };
    }
  }

  if (confirm?.value.type === "noul" && confirm.value.probability >= NOUL_CONFIRM_THRESHOLD) {
    return {
      action: "escalate",
      reason: `TDM judges the agent should ask first (P=${confirm.value.probability})`,
    };
  }

  return { action: "allow", reason: `TDM risk verdict: allow (floor ${policy.confidenceFloor})` };
}
