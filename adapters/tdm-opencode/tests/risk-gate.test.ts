import type {
  DecisionRequest,
  DecisionResult,
  JudgeOptions,
  TdmClient,
} from "@typedecision/client";
import { describe, expect, it, vi } from "vitest";

import { createRiskGate, type ExecuteBeforeEvent, type PermissionEvaluateEvent } from "../src/gate";

function safeResult(): DecisionResult {
  return {
    answers: [
      {
        id: "risk",
        value: { type: "score", level: "safe", weighted: 0, distribution: [["safe", 1]] },
        confidence: 0.95,
      },
      { id: "confirm", value: { type: "noul", probability: 0.05 }, confidence: null },
    ],
    usage: { inputTokens: 1, outputTokens: 1 },
    provider: { id: "mock", model: null },
    latencyMs: 1,
  };
}

function dangerousResult(): DecisionResult {
  return {
    answers: [
      {
        id: "risk",
        value: { type: "score", level: "dangerous", weighted: 3, distribution: [["dangerous", 1]] },
        confidence: 0.9,
      },
      { id: "confirm", value: { type: "noul", probability: 0.1 }, confidence: null },
    ],
    usage: { inputTokens: 1, outputTokens: 1 },
    provider: { id: "mock", model: null },
    latencyMs: 1,
  };
}

type CapturedCall = { req: DecisionRequest; opts?: JudgeOptions };

function fakeClient(
  judgeImpl: (req: DecisionRequest, opts?: JudgeOptions) => Promise<DecisionResult>,
): { client: TdmClient; calls: CapturedCall[] } {
  const calls: CapturedCall[] = [];
  return {
    calls,
    client: {
      judge: async (req, opts) => {
        calls.push({ req, opts });
        return judgeImpl(req, opts);
      },
      health: async () => {
        throw new Error("health is not exercised by gate tests");
      },
    },
  };
}

function beforeEvent(overrides: Partial<ExecuteBeforeEvent> = {}): ExecuteBeforeEvent {
  return {
    tool: "bash",
    sessionID: "ses_gate",
    id: "call_1",
    input: { command: "echo hi" },
    ...overrides,
  };
}

function evaluateEvent(overrides: Partial<PermissionEvaluateEvent> = {}): PermissionEvaluateEvent {
  return { effect: "allow", source: { type: "tool", id: "call_1" }, ...overrides };
}

describe("createRiskGate", () => {
  it("allows a safe verdict: permission effect stays untouched", async () => {
    const { client } = fakeClient(async () => safeResult());
    const gate = createRiskGate(client);

    await gate.onExecuteBefore(beforeEvent());
    const event = evaluateEvent();
    await gate.onPermissionEvaluate(event);

    expect(event.effect).toBe("allow");
    expect(event.message).toBeUndefined();
    expect(gate.stats()).toEqual({ judged: 1, escalated: 0, failed: 0 });
  });

  it("escalates a dangerous verdict: effect mutates to ask with the reason as message", async () => {
    const { client } = fakeClient(async () => dangerousResult());
    const gate = createRiskGate(client);

    await gate.onExecuteBefore(beforeEvent());
    const event = evaluateEvent();
    await gate.onPermissionEvaluate(event);

    expect(event.effect).toBe("ask");
    expect(event.message).toContain("dangerous");
    expect(gate.stats()).toEqual({ judged: 1, escalated: 1, failed: 0 });
  });

  it("escalates when the noul ask-first probability is high even on a safe score", async () => {
    const { client } = fakeClient(async () => ({
      answers: [
        {
          id: "risk",
          value: { type: "score", level: "safe", weighted: 0, distribution: [["safe", 1]] },
          confidence: 0.99,
        },
        { id: "confirm", value: { type: "noul", probability: 0.9 }, confidence: null },
      ],
      usage: { inputTokens: 1, outputTokens: 1 },
      provider: { id: "mock", model: null },
      latencyMs: 1,
    }));
    const gate = createRiskGate(client);

    await gate.onExecuteBefore(beforeEvent());
    const event = evaluateEvent();
    await gate.onPermissionEvaluate(event);

    expect(event.effect).toBe("ask");
    expect(event.message).toContain("ask first");
  });

  it("fails open on a client error: allows the call and warns via console.warn", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const { client } = fakeClient(async () => {
      throw new Error("native runtime missing");
    });
    const gate = createRiskGate(client);
    try {
      await gate.onExecuteBefore(beforeEvent());
      const event = evaluateEvent();
      await gate.onPermissionEvaluate(event);

      expect(event.effect).toBe("allow");
      expect(event.message).toBeUndefined();
      expect(gate.stats()).toEqual({ judged: 0, escalated: 0, failed: 1 });
      expect(warn).toHaveBeenCalledTimes(1);
      expect(warn.mock.calls[0]?.[0]).toContain("[tdm]");
      expect(warn.mock.calls[0]?.[0]).toContain("native runtime missing");
    } finally {
      warn.mockRestore();
    }
  });

  it("never judges allowlisted read-only tools", async () => {
    const { client, calls } = fakeClient(async () => safeResult());
    const gate = createRiskGate(client);

    await gate.onExecuteBefore(beforeEvent({ tool: "read", id: "call_read" }));
    await gate.onExecuteBefore(beforeEvent({ tool: "Grep", id: "call_grep" }));
    const event = evaluateEvent({ source: { type: "tool", id: "call_read" } });
    await gate.onPermissionEvaluate(event);

    expect(calls).toHaveLength(0);
    expect(event.effect).toBe("allow");
  });

  it("never judges its own tdm_judge tool (no self-judgment loops)", async () => {
    const { client, calls } = fakeClient(async () => safeResult());
    const gate = createRiskGate(client);

    await gate.onExecuteBefore(beforeEvent({ tool: "tdm_judge", id: "call_self" }));

    expect(calls).toHaveLength(0);
  });

  it("forwards the session context tagged with the opencode harness", async () => {
    const { client, calls } = fakeClient(async () => safeResult());
    const gate = createRiskGate(client);

    await gate.onExecuteBefore(beforeEvent({ sessionID: "ses_abc" }));
    expect(calls[0]?.opts?.session).toEqual({ harness: "opencode", sessionId: "ses_abc" });

    await gate.onExecuteBefore(beforeEvent({ sessionID: undefined, id: "call_2" }));
    expect(calls[1]?.opts?.session).toEqual({ harness: "opencode", sessionId: "unknown" });
  });

  it("builds the judgment state as { tool, args: <json string, ~2KB capped> }", async () => {
    const { client, calls } = fakeClient(async () => safeResult());
    const gate = createRiskGate(client);

    await gate.onExecuteBefore(beforeEvent({ input: { command: "rm -rf /" } }));
    expect(calls[0]?.req.state).toEqual({ tool: "bash", args: '{"command":"rm -rf /"}' });

    await gate.onExecuteBefore(beforeEvent({ id: "call_2", input: { blob: "y".repeat(8192) } }));
    const state = calls[1]?.req.state as { tool: string; args: string } | undefined;
    expect(typeof state?.args).toBe("string");
    expect(state?.args.length ?? Infinity).toBeLessThanOrEqual(2048 + 60);
    expect(state?.args).toContain("[truncated");
    expect(calls[1]?.req.questions.map((q) => q.id)).toEqual(["risk", "confirm"]);
  });

  it("passes through an uncorrelated permission event (no matching judgment)", async () => {
    const { client } = fakeClient(async () => dangerousResult());
    const gate = createRiskGate(client);

    const event = evaluateEvent({ source: { type: "tool", id: "call_unknown" } });
    await gate.onPermissionEvaluate(event);

    expect(event.effect).toBe("allow");
  });

  it("ignores permission events whose source is not a tool call", async () => {
    const { client } = fakeClient(async () => dangerousResult());
    const gate = createRiskGate(client);

    await gate.onExecuteBefore(beforeEvent());
    const event = evaluateEvent({ source: { type: "other", id: "call_1" } });
    await gate.onPermissionEvaluate(event);

    expect(event.effect).toBe("allow");
  });

  it("never downgrades an existing ask effect", async () => {
    const { client } = fakeClient(async () => safeResult());
    const gate = createRiskGate(client);

    await gate.onExecuteBefore(beforeEvent());
    const event = evaluateEvent({ effect: "ask" });
    await gate.onPermissionEvaluate(event);

    expect(event.effect).toBe("ask");
  });

  it("skips judging calls without a correlation id", async () => {
    const { client, calls } = fakeClient(async () => safeResult());
    const gate = createRiskGate(client);

    await gate.onExecuteBefore(beforeEvent({ id: undefined }));

    expect(calls).toHaveLength(0);
  });

  it("consumes a pending judgment exactly once", async () => {
    const { client } = fakeClient(async () => dangerousResult());
    const gate = createRiskGate(client);

    await gate.onExecuteBefore(beforeEvent());
    const first = evaluateEvent();
    await gate.onPermissionEvaluate(first);
    const second = evaluateEvent();
    await gate.onPermissionEvaluate(second);

    expect(first.effect).toBe("ask");
    expect(second.effect).toBe("allow"); // consumed: second pass-through
  });
});
