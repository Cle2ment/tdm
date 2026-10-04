import { tool } from "@opencode-ai/plugin";
import type { DecisionRequest, DecisionResult, TdmClient } from "@typedecision/client";
import { describe, expect, it } from "vitest";

import { judgeArgs } from "../src/args";
import { createJudgeHandler } from "../src/handler";

/**
 * Contract smoke fixture: a payload using all three primitives must satisfy
 * the generated DecisionRequest contract AND round-trip through the tool's
 * zod args shape (guards the schema against contract drift).
 */
const fixture = {
  state: {
    repo: "tdm",
    scores: [1, 2.5, null],
    meta: { nested: ["a", false] },
  },
  questions: [
    {
      id: "route",
      instructions: "Pick the owning team.",
      primitive: { type: "choice", options: ["core", "adapters", "infra"] },
    },
    {
      id: "is_breaking",
      instructions: "Does the change break the public contract?",
      primitive: { type: "noul" },
    },
    {
      id: "priority",
      instructions: "How urgent is this?",
      primitive: { type: "score", levels: ["low", "medium", "high", "critical"] },
    },
  ],
} satisfies DecisionRequest;

function capturingClient(result: DecisionResult): {
  client: TdmClient;
  received: DecisionRequest[];
} {
  const received: DecisionRequest[] = [];
  return {
    received,
    client: {
      judge: async (req) => {
        received.push(req);
        return result;
      },
      health: async () => {
        throw new Error("not used");
      },
    },
  };
}

const canned: DecisionResult = {
  answers: fixture.questions.map((q) => ({
    id: q.id,
    value: { type: "noul", probability: 0.5 },
    confidence: null,
  })),
  usage: { inputTokens: 1, outputTokens: 1 },
  provider: { id: "mock", model: null },
  latencyMs: 1,
};

describe("DecisionRequest contract fixture", () => {
  it("passes through the handler untouched (all three primitives)", async () => {
    const { client, received } = capturingClient(canned);
    await createJudgeHandler(client)(fixture);

    expect(received).toHaveLength(1);
    expect(received[0]).toEqual(fixture);
    expect(received[0].questions.map((q) => q.primitive.type)).toEqual(["choice", "noul", "score"]);
  });

  it("is accepted by the tool's zod args shape", () => {
    const parsed = tool.schema.object(judgeArgs).parse(fixture);
    expect(parsed.questions).toHaveLength(3);
    expect(parsed.questions[2].primitive).toEqual({
      type: "score",
      levels: ["low", "medium", "high", "critical"],
    });
  });

  it("rejects a malformed primitive at the args boundary", () => {
    const bad = {
      ...fixture,
      questions: [
        {
          id: "bad",
          instructions: "Invalid primitive shape.",
          primitive: { type: "choice", options: [] },
        },
      ],
    };
    expect(tool.schema.object(judgeArgs).safeParse(bad).success).toBe(false);
  });
});
