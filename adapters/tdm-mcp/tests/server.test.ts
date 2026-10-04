import { describe, expect, it } from "vitest";

import packageJson from "../package.json" with { type: "json" };
import { startServer } from "./helpers.ts";

describe("createTdmServer", () => {
  it("exposes exactly the three TDM tools", async () => {
    const { client, close } = await startServer();
    try {
      const { tools } = await client.listTools();
      expect(tools.map((tool) => tool.name).sort()).toEqual([
        "tdm_health",
        "tdm_judge",
        "tdm_providers",
      ]);
    } finally {
      await close();
    }
  });

  it("reports server info { name: 'tdm', version: from package.json }", async () => {
    const { client, close } = await startServer();
    try {
      expect(client.getServerVersion()).toEqual({
        name: "tdm",
        version: packageJson.version,
      });
    } finally {
      await close();
    }
  });

  it("documents the judge tool input schema (state + questions + routing fields)", async () => {
    const { client, close } = await startServer();
    try {
      const { tools } = await client.listTools();
      const judge = tools.find((tool) => tool.name === "tdm_judge");
      expect(judge).toBeDefined();
      const schema = judge?.inputSchema as {
        required?: Array<string>;
        properties?: Record<string, unknown>;
      };
      expect(schema.required).toEqual(expect.arrayContaining(["state", "questions"]));
      expect(Object.keys(schema.properties ?? {})).toEqual(
        expect.arrayContaining(["state", "questions", "provider", "session"]),
      );
    } finally {
      await close();
    }
  });
});
