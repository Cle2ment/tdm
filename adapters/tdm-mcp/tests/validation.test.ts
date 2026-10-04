import { describe, expect, it } from "vitest";

import { fakeClient, startServer, textOf } from "./helpers.ts";

describe("tdm_judge args validation (SDK-level, before any handler runs)", () => {
  it("rejects a malformed question primitive with validation error content", async () => {
    const fake = fakeClient();
    const { client, close } = await startServer({ client: fake });
    try {
      const result = await client.callTool({
        name: "tdm_judge",
        arguments: {
          state: {},
          questions: [
            {
              id: "bad",
              instructions: "Pick one.",
              primitive: { type: "choice", options: [] },
            },
          ],
        },
      });
      expect(result.isError).toBe(true);
      const text = textOf(result as never);
      expect(text).toContain("Invalid arguments for tool tdm_judge");
      expect(fake.calls).toHaveLength(0);
    } finally {
      await close();
    }
  });

  it("rejects an unknown primitive kind", async () => {
    const { client, close } = await startServer({ client: fakeClient() });
    try {
      const result = await client.callTool({
        name: "tdm_judge",
        arguments: {
          state: {},
          questions: [{ id: "q", instructions: "Judge.", primitive: { type: "essay" } }],
        },
      });
      expect(result.isError).toBe(true);
      expect(textOf(result as never)).toContain("Invalid arguments for tool tdm_judge");
    } finally {
      await close();
    }
  });

  it("rejects an empty questions array", async () => {
    const { client, close } = await startServer({ client: fakeClient() });
    try {
      const result = await client.callTool({
        name: "tdm_judge",
        arguments: { state: {}, questions: [] },
      });
      expect(result.isError).toBe(true);
      expect(textOf(result as never)).toContain("Invalid arguments for tool tdm_judge");
    } finally {
      await close();
    }
  });
});
