import type { DecisionResult } from "@typedecision/client";
import { describe, expect, it, vi } from "vitest";

import {
  ARGS_LIMIT,
  buildRiskRequest,
  DEFAULT_CONFIDENCE_FLOOR,
  evaluateRisk,
  needsJudgment,
  parsePolicy,
  truncateArgs,
} from "../src/policy";

/** Build a DecisionResult from the two verdict answers (score + noul). */
function makeResult(
  risk: { level: string; confidence: number | null } | null,
  confirm: { probability: number } | null,
): DecisionResult {
  const answers: DecisionResult["answers"] = [];
  if (risk) {
    answers.push({
      id: "risk",
      value: {
        type: "score",
        level: risk.level,
        weighted: 1,
        distribution: [[risk.level, 1]],
      },
      confidence: risk.confidence,
    });
  }
  if (confirm) {
    answers.push({
      id: "confirm",
      value: { type: "noul", probability: confirm.probability },
      confidence: null,
    });
  }
  return {
    answers,
    usage: { inputTokens: 1, outputTokens: 1 },
    provider: { id: "mock", model: null },
    latencyMs: 1,
  };
}

describe("needsJudgment", () => {
  it("exempts the plugin's own tdm_judge tool (no self-judgment loops)", () => {
    expect(needsJudgment("tdm_judge")).toBe(false);
  });

  it("exempts every allowlisted read-only tool", () => {
    for (const tool of ["read", "grep", "glob", "list", "ls", "skill", "webfetch", "websearch"]) {
      expect(needsJudgment(tool), tool).toBe(false);
    }
  });

  it("matches tool names case-insensitively (v2 names are lowercase but stay defensive)", () => {
    expect(needsJudgment("Read")).toBe(false);
    expect(needsJudgment("WebFetch")).toBe(false);
  });

  it("judges write-side tools and anything unknown", () => {
    for (const tool of ["bash", "edit", "write", "apply_patch", "some_mcp_tool"]) {
      expect(needsJudgment(tool), tool).toBe(true);
    }
  });
});

describe("truncateArgs", () => {
  it("passes short args through as JSON", () => {
    expect(truncateArgs({ command: "ls" })).toBe('{"command":"ls"}');
  });

  it("hard-truncates long args to ~2KB with a marker", () => {
    const huge = { blob: "x".repeat(ARGS_LIMIT * 4) };
    const out = truncateArgs(huge);
    expect(out.length).toBeLessThanOrEqual(ARGS_LIMIT + 60);
    expect(out).toContain("[truncated");
    expect(out.startsWith(`{"blob":"xxxx`)).toBe(true);
  });

  it("degrades unserializable args to a marker instead of throwing", () => {
    const cyclic: Record<string, unknown> = {};
    cyclic.self = cyclic;
    expect(truncateArgs(cyclic)).toBe("[unserializable tool args]");
  });

  it("handles undefined args", () => {
    expect(truncateArgs(undefined)).toBe("undefined");
  });
});

describe("buildRiskRequest", () => {
  it("carries tool + truncated args in state and the two spec questions", () => {
    const request = buildRiskRequest("bash", { command: "rm -rf /" });
    expect(request.state).toEqual({ tool: "bash", args: '{"command":"rm -rf /"}' });
    expect(request.questions).toHaveLength(2);

    const [score, noul] = request.questions;
    expect(score.id).toBe("risk");
    expect(score.instructions).toBe(
      "How risky is executing this tool call without asking the user?",
    );
    expect(score.primitive).toEqual({
      type: "score",
      levels: ["safe", "minor risk", "risky", "dangerous"],
    });
    expect(noul.id).toBe("confirm");
    expect(noul.instructions).toBe(
      "Should the agent ask the user for confirmation before executing?",
    );
    expect(noul.primitive).toEqual({ type: "noul" });
  });
});

describe("parsePolicy", () => {
  it("defaults the floor to 0.55", () => {
    expect(parsePolicy(undefined)).toEqual({ confidenceFloor: DEFAULT_CONFIDENCE_FLOOR });
    expect(parsePolicy({})).toEqual({ confidenceFloor: DEFAULT_CONFIDENCE_FLOOR });
  });

  it("accepts a floor override from plugin options", () => {
    expect(parsePolicy({ confidenceFloor: 0.9 })).toEqual({ confidenceFloor: 0.9 });
  });

  it("falls back to the default (with a warning) on invalid floors", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    try {
      for (const bad of [1.5, -0.1, Number.NaN, "0.7"]) {
        expect(parsePolicy({ confidenceFloor: bad as number })).toEqual({
          confidenceFloor: DEFAULT_CONFIDENCE_FLOOR,
        });
      }
      expect(warn).toHaveBeenCalledTimes(4);
      expect(warn.mock.calls[0]?.[0]).toContain("[tdm]");
    } finally {
      warn.mockRestore();
    }
  });
});

describe("evaluateRisk (policy matrix)", () => {
  const policy = { confidenceFloor: 0.55 };

  it("allows a safe, confident, non-confirming verdict", () => {
    const outcome = evaluateRisk(
      makeResult({ level: "safe", confidence: 0.9 }, { probability: 0.1 }),
      policy,
    );
    expect(outcome.action).toBe("allow");
  });

  it("allows a boundary confidence exactly at the floor", () => {
    const outcome = evaluateRisk(
      makeResult({ level: "risky", confidence: 0.55 }, { probability: 0.5 }),
      policy,
    );
    expect(outcome.action).toBe("allow");
  });

  it("escalates on confidence below the floor", () => {
    const outcome = evaluateRisk(
      makeResult({ level: "risky", confidence: 0.3 }, { probability: 0.2 }),
      policy,
    );
    expect(outcome.action).toBe("escalate");
    expect(outcome.reason).toContain("below the floor");
  });

  it("escalates on a dangerous level regardless of confidence", () => {
    const outcome = evaluateRisk(
      makeResult({ level: "dangerous", confidence: 0.99 }, { probability: 0.05 }),
      policy,
    );
    expect(outcome.action).toBe("escalate");
    expect(outcome.reason).toContain("dangerous");
  });

  it("escalates when the noul ask-first probability reaches 0.8", () => {
    const outcome = evaluateRisk(
      makeResult({ level: "safe", confidence: 0.99 }, { probability: 0.8 }),
      policy,
    );
    expect(outcome.action).toBe("escalate");
    expect(outcome.reason).toContain("ask first");
  });

  it("allows when the noul probability stays below the threshold", () => {
    const outcome = evaluateRisk(
      makeResult({ level: "minor risk", confidence: 0.8 }, { probability: 0.79 }),
      policy,
    );
    expect(outcome.action).toBe("allow");
  });

  it("escalates a dangerous verdict even when the judge is perfectly certain it is safe-ish elsewhere", () => {
    // dangerous + noul both firing: the dangerous branch wins (checked first).
    const outcome = evaluateRisk(
      makeResult({ level: "dangerous", confidence: 0.6 }, { probability: 0.9 }),
      policy,
    );
    expect(outcome.action).toBe("escalate");
    expect(outcome.reason).toContain("dangerous");
  });

  it("allows (fail-open) a verdict it cannot read", () => {
    const outcome = evaluateRisk(makeResult(null, null), policy);
    expect(outcome.action).toBe("allow");
  });

  it("honors a raised floor override", () => {
    const outcome = evaluateRisk(
      makeResult({ level: "risky", confidence: 0.7 }, { probability: 0.1 }),
      { confidenceFloor: 0.9 },
    );
    expect(outcome.action).toBe("escalate");
  });
});
