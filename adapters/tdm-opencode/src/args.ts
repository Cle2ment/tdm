/**
 * zod args shape for the `tdm_judge` tool, mirroring the TDM `DecisionRequest`
 * contract (adapters/tdm-contract, generated from backend/tdm-core — D7).
 *
 * The opencode plugin SDK's `tool()` helper consumes a zod raw shape here and
 * the host converts it to the tool JSON schema for the LLM. zod is pinned to
 * the exact version @opencode-ai/plugin depends on, so host and plugin share
 * a single zod instance.
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

/** Tool args = the DecisionRequest contract, as a zod raw shape. */
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
};
