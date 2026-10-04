/**
 * Shared test helpers: a scriptable fake TdmClient (no native module, no
 * network) and an in-memory MCP link (SDK InMemoryTransport + real Client).
 */

import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import type { CallToolResult } from "@modelcontextprotocol/sdk/types.js";
import type {
  DecisionRequest,
  DecisionResult,
  HealthReport,
  JudgeOptions,
  TdmClient,
} from "@typedecision/client";

import { createTdmServer, type TdmServerOptions } from "../src/server.ts";

export const cannedResult: DecisionResult = {
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
  ],
  usage: { inputTokens: 120, outputTokens: 40 },
  provider: { id: "mock", model: null },
  latencyMs: 42,
};

export const cannedHealth: HealthReport = {
  ok: true,
  latencyMs: 7,
  detail: null,
  version: "0.1.0",
};

export type CapturedJudgeCall = { req: DecisionRequest; opts?: JudgeOptions };

export interface FakeClient extends TdmClient {
  calls: Array<CapturedJudgeCall>;
}

/** Scriptable TdmClient fake: records judge calls, delegates judge/health to impls. */
export function fakeClient(
  overrides: {
    judge?: (req: DecisionRequest, opts?: JudgeOptions) => Promise<DecisionResult>;
    health?: () => Promise<HealthReport>;
  } = {},
): FakeClient {
  const calls: Array<CapturedJudgeCall> = [];
  return {
    calls,
    judge: async (req, opts) => {
      calls.push({ req, opts });
      const impl = overrides.judge;
      if (impl) return impl(req, opts);
      return cannedResult;
    },
    health: overrides.health ?? (async () => cannedHealth),
  };
}

/** Wire a fresh tdm server to a real MCP client over the in-memory transport pair. */
export async function startServer(options: TdmServerOptions = {}): Promise<{
  client: Client;
  close: () => Promise<void>;
}> {
  const server: McpServer = createTdmServer(options);
  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
  const client = new Client({ name: "test-client", version: "0.0.0" });
  await server.connect(serverTransport);
  await client.connect(clientTransport);
  return {
    client,
    close: async () => {
      await client.close();
      await server.close();
    },
  };
}

/** First text block of a tool result, typed for assertions. */
export function textOf(result: CallToolResult): string {
  const first = result.content[0] as { type: string; text?: string } | undefined;
  if (first?.type !== "text" || typeof first.text !== "string") {
    throw new Error(`expected a text content block, got ${JSON.stringify(result.content)}`);
  }
  return first.text;
}
