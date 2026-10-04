/**
 * TDM harness adapter for opencode (M0 tracer bullet): registers one custom
 * tool, `tdm_judge`, so the agent can run typed judgments through TDM.
 *
 * Plugin shape matches opencode v2 hosts: the module default-exports a
 * `Plugin` *definition object* — `{ id, setup }` — and the host invokes
 * `setup(ctx)` exactly once. `tdm_judge` is registered through
 * `ctx.tool.transform(editor => editor.add({...}))`, where tools take a
 * JSON Schema `input` (zod → `z.toJSONSchema`) and an async `execute`
 * returning `{ content }`. `setup` returns a cleanup that disposes the
 * registration. The SDK package is imported type-only, so the plugin ships
 * no runtime SDK dependency (the host supplies the behavior). Automatic
 * risk scoring via hooks is milestone M2.
 */
import type { Plugin } from "@opencode/plugin";
import type { TdmClient } from "@typedecision/client";
import { createClient } from "@typedecision/client";
import { z } from "zod";

import { judgeArgs } from "./args";
import { createJudgeHandler } from "./handler";

export type { JudgeCallContext, JudgeHandler } from "./handler";
export { createJudgeHandler } from "./handler";

const TOOL_DESCRIPTION = `Run typed judgments through TDM for structured micro-decisions — routing, classification, verification, scoring — where calibrated, auditable answers beat free-form guessing.

Pass \`state\` (any JSON carrying the full decision context) plus one or more independent \`questions\`. Each question pairs self-contained \`instructions\` with one of three primitives:
- {"type":"choice","options":[...]} — pick exactly one option label;
- {"type":"noul"} — yes/no verification; the answer carries P(yes);
- {"type":"score","levels":[...]} — pick exactly one ordered level label.

Answers are typed and probabilistic: every answer carries a confidence, and choice/score answers include the full (label, probability) distribution. Prefer this tool over deciding inline whenever the judgment feeds code logic.`;

/**
 * Client singleton, created on the first tool call — mirrors NapiClient's lazy
 * native-module load and keeps plugin load free of side effects.
 */
let cachedClient: TdmClient | undefined;

function getClient(): TdmClient {
  cachedClient ??= createClient({ mode: "auto" });
  return cachedClient;
}

/** The DecisionRequest contract as a validating zod schema. */
const judgeRequest = z.object(judgeArgs);

/** LLM-facing tool input schema, derived once at module load. */
const judgeInputSchema = z.toJSONSchema(judgeRequest);

export default {
  id: "tdm",
  setup: async (context) => {
    const judge = createJudgeHandler(getClient());
    const registration = await context.tool.transform((editor) => {
      editor.add({
        name: "tdm_judge",
        description: TOOL_DESCRIPTION,
        input: judgeInputSchema,
        options: { codemode: false },
        execute: async (args, toolContext) => {
          const parsed = judgeRequest.safeParse(args);
          if (!parsed.success) {
            return {
              content: `TDM judge rejected the arguments: ${parsed.error.message}`,
            };
          }
          return { content: await judge(parsed.data, toolContext) };
        },
      });
    });
    return async () => {
      await registration.dispose();
    };
  },
} satisfies Plugin.Plugin;
