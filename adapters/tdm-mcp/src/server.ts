/**
 * tdm-mcp server factory: wires the three TDM tools onto the MCP SDK's
 * high-level `McpServer` (D5 — universal path + secrets barrier).
 *
 * The only MCP surface is exactly three tools (`tdm_judge`, `tdm_health`,
 * `tdm_providers`); everything else — the TdmClient, its native runtime, and
 * any provider API key — stays inside this server process.
 *
 * Handlers never throw to the transport: client failures come back as
 * `isError` text content carrying the client's message. (The SDK itself also
 * converts handler throws to error results; we catch explicitly to keep the
 * message format ours.)
 */

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import type { CallToolResult } from "@modelcontextprotocol/sdk/types.js";
import type { TdmClient } from "@typedecision/client";

import packageJson from "../package.json" with { type: "json" };
import { judgeArgs } from "./args.ts";
import { createLazyClient, resolveProviderInfo } from "./client.ts";

export { judgeArgs } from "./args.ts";
export { resolveDefaultProviderId, resolveProviderInfo } from "./client.ts";

/** Reuse the opencode adapter's LLM-facing pitch; same contract underneath. */
const JUDGE_DESCRIPTION = `Run typed judgments through TDM for structured micro-decisions — routing, classification, verification, scoring — where calibrated, auditable answers beat free-form guessing.

Pass \`state\` (any JSON carrying the full decision context) plus one or more independent \`questions\`. Each question pairs self-contained \`instructions\` with one of three primitives:
- {"type":"choice","options":[...]} — pick exactly one option label;
- {"type":"noul"} — yes/no verification; the answer carries P(yes);
- {"type":"score","levels":[...]} — pick exactly one ordered level label.

Answers are typed and probabilistic: every answer carries a confidence, and choice/score answers include the full (label, probability) distribution. Prefer this tool over deciding inline whenever the judgment feeds code logic.`;

function textResult(payload: unknown): CallToolResult {
  return { content: [{ type: "text", text: JSON.stringify(payload, null, 2) }] };
}

function errorResult(header: string, cause: unknown): CallToolResult {
  const message = cause instanceof Error ? cause.message : String(cause);
  return { content: [{ type: "text", text: `${header}: ${message}` }], isError: true };
}

export interface TdmServerOptions {
  /** Injected client (tests); defaults to a lazy auto-transport client. */
  client?: TdmClient;
  /** Env source for provider resolution (tests); defaults to process.env. */
  env?: Record<string, string | undefined>;
}

/** Server info name reported over the MCP `initialize` handshake. */
const SERVER_NAME = "tdm";

/**
 * Build the tdm MCP server. No I/O happens here — attach a transport via
 * `server.connect(...)`; the bin entry (src/cli.ts) uses StdioServerTransport.
 */
export function createTdmServer(options: TdmServerOptions = {}): McpServer {
  const server = new McpServer({ name: SERVER_NAME, version: packageJson.version });
  const injected = options.client;
  const getClient: () => TdmClient = injected ? () => injected : createLazyClient().get;
  const env = options.env;

  server.registerTool(
    "tdm_judge",
    { description: JUDGE_DESCRIPTION, inputSchema: judgeArgs },
    async (args, extra) => {
      try {
        // Contract fidelity: DecisionRequest is exactly { state, questions };
        // the routing fields ride along in JudgeOptions only.
        const request = { state: args.state, questions: args.questions };
        const result = await getClient().judge(request, {
          provider: args.provider,
          session: {
            harness: args.session?.harness ?? "mcp",
            sessionId: args.session?.sessionId ?? extra.sessionId ?? "unknown",
          },
        });
        return textResult(result);
      } catch (cause) {
        return errorResult("TDM judge failed", cause);
      }
    },
  );

  server.registerTool(
    "tdm_health",
    {
      description: "Probe the configured TDM judgment provider for liveness/connectivity.",
    },
    async () => {
      try {
        return textResult(await getClient().health());
      } catch (cause) {
        return errorResult("TDM health probe failed", cause);
      }
    },
  );

  server.registerTool(
    "tdm_providers",
    {
      description:
        "List the judgment provider ids this TDM server can use, and which one is the default. " +
        "M1: reports the env-resolved default provider (TDM_JEV_API_KEY/TYPESAFE_API_KEY → jev, else mock).",
    },
    async () => textResult(resolveProviderInfo(env)),
  );

  return server;
}
