import { describe, expect, it } from "vitest";

import { cannedHealth, type cannedResult, fakeClient, startServer, textOf } from "./helpers.ts";

const validArgs = {
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
  ],
};

describe("tdm_judge via the MCP wire", () => {
  it("returns the DecisionResult with typed answers", async () => {
    const { client, close } = await startServer({ client: fakeClient() });
    try {
      const result = await client.callTool({ name: "tdm_judge", arguments: validArgs });
      expect(result.isError).toBeUndefined();

      const parsed = JSON.parse(textOf(result as never)) as typeof cannedResult;
      expect(parsed.answers.map((a) => a.id)).toEqual(["route", "guard"]);
      expect(parsed.answers[0].value).toEqual({
        type: "choice",
        label: "billing",
        distribution: [
          ["billing", 0.9],
          ["support", 0.1],
        ],
      });
      expect(parsed.answers[1].value).toEqual({ type: "noul", probability: 0.98 });
      expect(parsed.provider).toEqual({ id: "mock", model: null });
    } finally {
      await close();
    }
  });

  it("forwards the request and routing fields, defaulting the harness to 'mcp'", async () => {
    const fake = fakeClient();
    const { client, close } = await startServer({ client: fake });
    try {
      await client.callTool({
        name: "tdm_judge",
        arguments: { ...validArgs, provider: "jev", session: { sessionId: "ses_x" } },
      });
      expect(fake.calls).toHaveLength(1);
      expect(fake.calls[0].req).toEqual(validArgs);
      expect(fake.calls[0].opts).toEqual({
        provider: "jev",
        session: { harness: "mcp", sessionId: "ses_x" },
      });
    } finally {
      await close();
    }
  });

  it("converts client failures into isError content instead of throwing", async () => {
    const { client, close } = await startServer({
      client: fakeClient({
        judge: async () => {
          throw new Error('Failed to load the TDM native runtime from "@typedecision/runtime".');
        },
      }),
    });
    try {
      const result = await client.callTool({ name: "tdm_judge", arguments: validArgs });
      expect(result.isError).toBe(true);
      const text = textOf(result as never);
      expect(text).toMatch(/^TDM judge failed: /);
      expect(text).toContain("Failed to load the TDM native runtime");
    } finally {
      await close();
    }
  });
});

describe("tdm_health via the MCP wire", () => {
  it("passes the provider health report through", async () => {
    const { client, close } = await startServer({ client: fakeClient() });
    try {
      const result = await client.callTool({ name: "tdm_health", arguments: {} });
      expect(result.isError).toBeUndefined();
      expect(JSON.parse(textOf(result as never))).toEqual(cannedHealth);
    } finally {
      await close();
    }
  });

  it("reports provider outages as isError content, not transport errors", async () => {
    const { client, close } = await startServer({
      client: fakeClient({
        health: async () => {
          throw new Error("provider unreachable");
        },
      }),
    });
    try {
      const result = await client.callTool({ name: "tdm_health", arguments: {} });
      expect(result.isError).toBe(true);
      expect(textOf(result as never)).toBe("TDM health probe failed: provider unreachable");
    } finally {
      await close();
    }
  });
});

describe("tdm_providers via the MCP wire", () => {
  it("resolves jev when TDM_JEV_API_KEY is set", async () => {
    const { client, close } = await startServer({ env: { TDM_JEV_API_KEY: "k" } });
    try {
      const result = await client.callTool({ name: "tdm_providers", arguments: {} });
      expect(JSON.parse(textOf(result as never))).toEqual({
        default: "jev",
        providers: ["jev"],
        source: "env",
      });
    } finally {
      await close();
    }
  });

  it("resolves jev when TYPESAFE_API_KEY is set", async () => {
    const { client, close } = await startServer({ env: { TYPESAFE_API_KEY: "k" } });
    try {
      const result = await client.callTool({ name: "tdm_providers", arguments: {} });
      expect(JSON.parse(textOf(result as never))).toMatchObject({ default: "jev" });
    } finally {
      await close();
    }
  });

  it("falls back to mock without any key", async () => {
    const { client, close } = await startServer({ env: {} });
    try {
      const result = await client.callTool({ name: "tdm_providers", arguments: {} });
      expect(JSON.parse(textOf(result as never))).toEqual({
        default: "mock",
        providers: ["mock"],
        source: "env",
      });
    } finally {
      await close();
    }
  });
});
