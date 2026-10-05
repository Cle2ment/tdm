import type { DecisionRequest, DecisionResult, HealthReport } from "@typedecision/contract";
import { describe, expect, it, vi } from "vitest";
import { type JudgeOptions, NapiClient } from "../src/index";

const runtime = vi.hoisted(() => ({
  judge: vi.fn(),
  health: vi.fn(),
}));

vi.mock("@typedecision/runtime", () => runtime);

const REQUEST = {
  state: { path: "src/main.rs", action: "delete_file" },
  questions: [
    {
      id: "proceed",
      instructions: "Proceed with this deletion without user confirmation?",
      primitive: { type: "noul" },
    },
  ],
} satisfies DecisionRequest;

const RESULT: DecisionResult = {
  answers: [{ id: "proceed", value: { type: "noul", probability: 0.12 }, confidence: null }],
  usage: { inputTokens: 84, outputTokens: 16 },
  provider: { id: "mock", model: null },
  latencyMs: 7,
};

const REPORT: HealthReport = { ok: true, latencyMs: 3, detail: null, version: "0.1.0" };

describe("NapiClient passthrough", () => {
  it("forwards judge(req, opts) to the native module and returns its result", async () => {
    runtime.judge.mockResolvedValue(RESULT);
    const client = new NapiClient();
    const opts: JudgeOptions = {
      session: { harness: "opencode", sessionId: "ses_123" },
      provider: "jev",
    };

    await expect(client.judge(REQUEST, opts)).resolves.toEqual(RESULT);
    expect(runtime.judge).toHaveBeenCalledExactlyOnceWith(REQUEST, opts);
  });

  it("allows calling judge() without opts", async () => {
    runtime.judge.mockResolvedValue(RESULT);
    const client = new NapiClient();

    await expect(client.judge(REQUEST)).resolves.toEqual(RESULT);
    expect(runtime.judge).toHaveBeenCalledExactlyOnceWith(REQUEST, undefined);
  });

  it("forwards health() to the native module and returns its report", async () => {
    runtime.health.mockResolvedValue(REPORT);
    const client = new NapiClient();

    await expect(client.health()).resolves.toEqual(REPORT);
    expect(runtime.health).toHaveBeenCalledExactlyOnceWith();
  });
});
