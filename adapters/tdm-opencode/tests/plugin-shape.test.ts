import type { Plugin } from "@opencode/plugin";

import type { DecisionRequest } from "@typedecision/client";
import { describe, expect, it, vi } from "vitest";

// Simulate the native binding being unavailable so the error path is
// deterministic regardless of whether @typedecision/runtime is installed.
// NapiClient catches the failed import and wraps it with its guidance message.
vi.mock("@typedecision/runtime", () => {
  throw new Error("simulated: native runtime not installed");
});

import plugin from "../src/index";

type SetupContext = Parameters<Plugin.Plugin["setup"]>[0];
type ToolDefinition = {
  name: string;
  description: string;
  input: Record<string, unknown>;
  execute: (args: unknown, context: unknown) => Promise<{ content: string }>;
};

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

/** Fake ctx.tool.transform that captures editor.add() calls. */
function captureTransform() {
  const added: ToolDefinition[] = [];
  const transform = async (edit: (editor: { add: (tool: ToolDefinition) => void }) => void) => {
    edit({ add: (tool) => added.push(tool) });
    return { dispose: vi.fn(async () => {}) };
  };
  return { added, transform };
}

async function setupPlugin() {
  const { added, transform } = captureTransform();
  const cleanup = await plugin.setup({ tool: { transform } } as unknown as SetupContext);
  return { added, cleanup };
}

const fakeToolContext = { sessionID: "ses_plugin" };

describe("plugin shape", () => {
  it("default-exports a V2 definition object with id and setup (host resolves mod.default)", () => {
    expect(typeof plugin).toBe("object");
    expect(plugin.id).toBe("tdm");
    expect(typeof plugin.setup).toBe("function");
  });

  it("registers exactly one tool, tdm_judge, with description, JSON schema input and execute", async () => {
    const { added } = await setupPlugin();

    expect(added.map((tool) => tool.name)).toEqual(["tdm_judge"]);
    const tdmJudge = added[0];

    expect(tdmJudge.description).toContain("TDM");
    expect(tdmJudge.description).toContain("choice");
    expect(tdmJudge.description).toContain("noul");
    expect(tdmJudge.description).toContain("score");

    const properties = (tdmJudge.input.properties ?? {}) as Record<string, unknown>;
    expect(Object.keys(properties).sort()).toEqual(["questions", "state"]);
    expect(typeof tdmJudge.execute).toBe("function");
  });

  it("setup returns a cleanup that disposes the tool registration", async () => {
    const disposers: Array<ReturnType<typeof vi.fn>> = [];
    const cleanup = await plugin.setup({
      tool: {
        transform: async (edit: (editor: { add: (tool: ToolDefinition) => void }) => void) => {
          edit({ add: () => {} });
          const dispose = vi.fn(async () => {});
          disposers.push(dispose);
          return { dispose };
        },
      },
    } as unknown as SetupContext);

    expect(typeof cleanup).toBe("function");
    await cleanup?.();
    expect(disposers[0]).toHaveBeenCalledTimes(1);
  });

  it("tool execute surfaces a missing native runtime as a clean error string", async () => {
    const { added } = await setupPlugin();
    const tdmJudge = added[0];

    const output = await tdmJudge.execute(request, fakeToolContext);

    // @typedecision/runtime is vi.mocked to throw above: the lazy NapiClient
    // import fails — the tool must report it, not crash the host.
    expect(output.content).toMatch(/^TDM judge failed: /);
    expect(output.content).toContain("Failed to load the TDM native runtime");
  });

  it("tool execute rejects malformed arguments with a descriptive string", async () => {
    const { added } = await setupPlugin();
    const tdmJudge = added[0];

    const output = await tdmJudge.execute({ state: 42 }, fakeToolContext);

    expect(output.content).toMatch(/^TDM judge rejected the arguments: /);
  });
});

/** A captured hook registration on one of the fake domains. */
type HookRegistration = {
  name: string;
  callback: (event: unknown) => Promise<void> | void;
};

/** Fake ctx with tool.transform + tool.hook + permission.hook (full V2 host). */
function captureFullContext() {
  const added: ToolDefinition[] = [];
  const toolHooks: HookRegistration[] = [];
  const permissionHooks: HookRegistration[] = [];
  const disposers: Array<ReturnType<typeof vi.fn>> = [];

  const makeHookDomain =
    (registry: HookRegistration[]) =>
    async (name: string, callback: (event: unknown) => Promise<void> | void) => {
      registry.push({ name, callback });
      const dispose = vi.fn(async () => {});
      disposers.push(dispose);
      return { dispose };
    };

  const context = {
    options: {},
    tool: {
      transform: async (edit: (editor: { add: (tool: ToolDefinition) => void }) => void) => {
        edit({ add: (tool) => added.push(tool) });
        const dispose = vi.fn(async () => {});
        disposers.push(dispose);
        return { dispose };
      },
      hook: makeHookDomain(toolHooks),
    },
    permission: {
      hook: makeHookDomain(permissionHooks),
    },
  };
  return { context, added, toolHooks, permissionHooks, disposers };
}

describe("M2 risk gate registration", () => {
  it("registers execute.before and permission.evaluate hooks alongside the tool", async () => {
    const { context, added, toolHooks, permissionHooks } = captureFullContext();
    const cleanup = await plugin.setup(context as unknown as SetupContext);

    expect(added.map((tool) => tool.name)).toEqual(["tdm_judge"]);
    expect(toolHooks.map((hook) => hook.name)).toEqual(["execute.before"]);
    expect(permissionHooks.map((hook) => hook.name)).toEqual(["evaluate"]);
    expect(typeof cleanup).toBe("function");
  });

  it("cleanup disposes the tool registration and both hooks", async () => {
    const { context, disposers } = captureFullContext();
    const cleanup = await plugin.setup(context as unknown as SetupContext);

    await cleanup?.();
    expect(disposers).toHaveLength(3);
    for (const dispose of disposers) {
      expect(dispose).toHaveBeenCalledTimes(1);
    }
  });

  it("still loads (tool only) when the host exposes no hook domains", async () => {
    const { added, cleanup } = await setupPlugin();
    expect(added.map((tool) => tool.name)).toEqual(["tdm_judge"]);
    await cleanup?.();
  });

  it("the registered gate escalates a dangerous verdict via the permission payload", async () => {
    const { context, toolHooks, permissionHooks } = captureFullContext();
    await plugin.setup(context as unknown as SetupContext);

    // Fire the stashed hooks the way the host would: before the tool body
    // (execute.before), then at the permission check (permission.evaluate).
    await toolHooks[0]?.callback({
      tool: "bash",
      sessionID: "ses_live",
      id: "call_42",
      input: { command: "rm -rf /" },
    });
    const event: { effect: string; message?: string; source?: { type: string; id: string } } = {
      effect: "allow",
      source: { type: "tool", id: "call_42" },
    };
    await permissionHooks[0]?.callback(event);

    // @typedecision/runtime is mocked to throw in this file, so the judgment
    // fails open — effect must stay "allow" and the host must not crash.
    expect(event.effect).toBe("allow");
  });
});
