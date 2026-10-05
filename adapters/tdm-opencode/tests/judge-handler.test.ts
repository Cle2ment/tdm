import type {
  DecisionRequest,
  DecisionResult,
  JudgeOptions,
  TdmClient,
} from "@typedecision/client";
import { describe, expect, it } from "vitest";

import { createJudgeHandler } from "../src/handler";

const request: DecisionRequest = {
  state: { ticket: "T-1", channel: "email" },
  questions: [
    {
      id: "route",
      instructions: "Route the ticket to the right team.",
      primitive: { type: "choice", options: ["billing", "support"] },
    },
    {
      id: "guard",
      instructions: "Is the customer angry?",
      primitive: { type: "noul" },
    },
    {
      id: "quality",
      instructions: "Rate the draft reply.",
      primitive: { type: "score", levels: ["bad", "ok", "great"] },
    },
  ],
};

const cannedResult: DecisionResult = {
  answers: [
    {
      id: "route",
      value: {
        type: "choice",
        label: "billing",
        distribution: [
          ["billing", 0.9],
          ["support", 0.1],
        ],
      },
      confidence: 0.9,
    },
    {
      id: "guard",
      value: { type: "noul", probability: 0.98 },
      confidence: null,
    },
    {
      id: "quality",
      value: {
        type: "score",
        level: "great",
        weighted: 2.7,
        distribution: [
          ["bad", 0.05],
          ["ok", 0.25],
          ["great", 0.7],
        ],
      },
      confidence: 0.7,
    },
  ],
  usage: { inputTokens: 120, outputTokens: 40 },
  provider: { id: "mock", model: null },
  latencyMs: 42,
};

type CapturedCall = { req: DecisionRequest; opts?: JudgeOptions };

function fakeClient(
  judgeImpl: (req: DecisionRequest, opts?: JudgeOptions) => Promise<DecisionResult>,
): { client: TdmClient; calls: CapturedCall[] } {
  const calls: CapturedCall[] = [];
  const client: TdmClient = {
    judge: async (req, opts) => {
      calls.push({ req, opts });
      return judgeImpl(req, opts);
    },
    health: async () => {
      throw new Error("health is not exercised by judge tests");
    },
  };
  return { client, calls };
}

describe("createJudgeHandler", () => {
  it("returns the DecisionResult with typed answers (ids and values)", async () => {
    const { client } = fakeClient(async () => cannedResult);
    const output = await createJudgeHandler(client)(request, { sessionID: "ses_out" });

    const parsed = JSON.parse(output) as DecisionResult;
    expect(parsed.answers.map((a) => a.id)).toEqual(["route", "guard", "quality"]);
    expect(parsed.answers[0].value).toEqual({
      type: "choice",
      label: "billing",
      distribution: [
        ["billing", 0.9],
        ["support", 0.1],
      ],
    });
    expect(parsed.answers[1].value).toEqual({ type: "noul", probability: 0.98 });
    expect(parsed.answers[2].value).toEqual({
      type: "score",
      level: "great",
      weighted: 2.7,
      distribution: [
        ["bad", 0.05],
        ["ok", 0.25],
        ["great", 0.7],
      ],
    });
  });

  it("forwards the request unchanged to the client", async () => {
    const { client, calls } = fakeClient(async () => cannedResult);
    await createJudgeHandler(client)(request);
    expect(calls).toHaveLength(1);
    expect(calls[0].req).toEqual(request);
  });

  it("converts client failures into a clean error string instead of throwing", async () => {
    const { client } = fakeClient(async () => {
      throw new Error('Failed to load the TDM native runtime from "@typedecision/runtime".');
    });

    const output = await createJudgeHandler(client)(request);
    expect(output).toMatch(/^TDM judge failed: /);
    expect(output).toContain("Failed to load the TDM native runtime");
  });

  it("stringifies non-Error throwables as well", async () => {
    const { client } = fakeClient(async () => {
      throw "boom";
    });

    const output = await createJudgeHandler(client)(request);
    expect(output).toBe("TDM judge failed: boom");
  });

  it("tags the call with the opencode harness and the session id", async () => {
    const { client, calls } = fakeClient(async () => cannedResult);
    await createJudgeHandler(client)(request, { sessionID: "ses_abc" });
    expect(calls[0].opts?.session).toEqual({
      harness: "opencode",
      sessionId: "ses_abc",
    });
  });

  it("falls back to sessionId 'unknown' outside a session", async () => {
    const { client, calls } = fakeClient(async () => cannedResult);
    await createJudgeHandler(client)(request);
    expect(calls[0].opts?.session).toEqual({
      harness: "opencode",
      sessionId: "unknown",
    });
  });
});
