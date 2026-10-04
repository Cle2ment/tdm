/**
 * zod args shapes for the tdm-mcp tools, mirroring the TDM `DecisionRequest`
 * contract (adapters/tdm-contract, generated from backend/tdm-core — D7).
 *
 * Same approach as adapters/tdm-opencode/src/args.ts: raw zod shapes handed to
 * the MCP SDK's `registerTool` (`inputSchema`), which derives the tool's JSON
 * Schema and validates every call before the handler runs. zod is pinned to
 * 3.25.76 — the exact version inside @modelcontextprotocol/sdk's peer range —
 * so this package and the SDK share a single zod instance.
 */

import type { JsonValue } from "@typedecision/contract";
import { z } from "zod";

/** Recursive JSON schema for `state`; annotated to break type inference. */
const jsonValue: z.ZodType<JsonValue> = z.lazy(() =>
  z.union([
    z.string(),
    z.number(),
    z.boolean(),
    z.null(),
    z.array(jsonValue),
    z.record(z.string(), jsonValue),
  ]),
);

const primitive = z.union([
  z.object({
    type: z.literal("choice"),
    options: z
      .array(z.string())
      .min(1)
      .max(255)
      .describe("Candidate option labels; the answer picks exactly one."),
  }),
  z.object({
    type: z.literal("noul"),
  }),
  z.object({
    type: z.literal("score"),
    levels: z
      .array(z.string())
      .min(2)
      .max(10)
      .describe("Ordered level labels, worst first; the answer picks exactly one."),
  }),
]);

/** `tdm_judge` args = the DecisionRequest contract + optional routing fields. */
export const judgeArgs = {
  state: jsonValue.describe("JSON value carrying all context the judgments need (decision state)."),
  questions: z
    .array(
      z.object({
        id: z
          .string()
          .describe(
            "Code-side identifier; echoed back verbatim in the matching answer, never sent to the judge model.",
          ),
        instructions: z
          .string()
          .describe(
            "Judgment instructions; must be semantically self-contained (no references to prior conversation).",
          ),
        primitive,
      }),
    )
    .min(1)
    .describe("Independent questions, judged in parallel against `state`."),
  provider: z
    .string()
    .min(1)
    .optional()
    .describe(
      'Optional provider override (e.g. "jev", "mock"); defaults to the server\'s configured provider.',
    ),
  session: z
    .object({
      harness: z.string().min(1).optional().describe('Audit harness tag; defaults to "mcp".'),
      sessionId: z
        .string()
        .min(1)
        .optional()
        .describe(
          "Audit session id; defaults to the MCP transport session id, if the host provides one.",
        ),
    })
    .optional()
    .describe("Optional audit identity override for cost/stats tagging."),
};

/** `tdm_health` / `tdm_providers` take no arguments. */
export const emptyArgs = {};
