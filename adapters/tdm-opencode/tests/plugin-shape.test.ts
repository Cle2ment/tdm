import type { PluginInput, ToolContext } from "@opencode-ai/plugin";

import type { DecisionRequest } from "@typedecision/client";
import { describe, expect, it, vi } from "vitest";

// Simulate the native binding being unavailable so the error path is
// deterministic regardless of whether @typedecision/runtime is installed.
// NapiClient catches the failed import and wraps it with its guidance message.
vi.mock("@typedecision/runtime", () => {
  throw new Error("simulated: native runtime not installed");
});

import plugin from "../src/index";

const request: DecisionRequest = {
  state: { probe: true },
  questions: [
    {
      id: "q1",
      instructions: "Anything goes.",
      primitive: { type: "noul" },
    },
  ],
};

const fakeInput = {} as PluginInput;

const fakeToolContext = {
  sessionID: "ses_plugin",
  messageID: "msg_plugin",
  agent: "test",
  directory: ".",
  worktree: ".",
  abort: new AbortController().signal,
  metadata: () => {},
  ask: async () => {},
} as ToolContext;

describe("plugin shape", () => {
  it("default-exports the Plugin function (host resolves mod.default)", () => {
    expect(typeof plugin).toBe("function");
  });

  it("registers exactly one tool, tdm_judge, with description, args and execute", async () => {
    const hooks = await plugin(fakeInput);
    const tdmJudge = hooks.tool?.tdm_judge;

    expect(Object.keys(hooks.tool ?? {})).toEqual(["tdm_judge"]);
    expect(tdmJudge).toBeDefined();
    if (!tdmJudge) throw new Error("tdm_judge tool was not registered");

    expect(tdmJudge.description).toContain("TDM");
    expect(tdmJudge.description).toContain("choice");
    expect(tdmJudge.description).toContain("noul");
    expect(tdmJudge.description).toContain("score");
    expect(Object.keys(tdmJudge.args).sort()).toEqual(["questions", "state"]);
    expect(typeof tdmJudge.execute).toBe("function");
  });

  it("initializes without side effects (lazy client, no native load)", async () => {
    await expect(plugin(fakeInput)).resolves.toBeDefined();
  });

  it("tool execute surfaces a missing native runtime as a clean error string", async () => {
    const hooks = await plugin(fakeInput);
    const tdmJudge = hooks.tool?.tdm_judge;
    if (!tdmJudge) throw new Error("tdm_judge tool was not registered");

    const output = await tdmJudge.execute(request, fakeToolContext);

    // @typedecision/runtime is vi.mocked to throw above: the lazy NapiClient
    // import fails — the tool must report it, not crash the host.
    expect(output).toMatch(/^TDM judge failed: /);
    expect(output).toContain("Failed to load the TDM native runtime");
  });
});
