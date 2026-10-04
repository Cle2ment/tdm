/**
 * TDM harness adapter for opencode (M0 tracer bullet): registers one custom
 * tool, `tdm_judge`, so the agent can run typed judgments through TDM.
 *
 * Plugin shape matches the installed opencode host: the module default-exports
 * a `Plugin` — `(input: PluginInput, options?) => Promise<Hooks>` — and the
 * host resolves exactly `mod.default` when loading a plugin module. No hooks
 * are used yet; automatic risk scoring via hooks is milestone M2.
 */
import type { Plugin } from "@opencode-ai/plugin";
import { tool } from "@opencode-ai/plugin";
import type { TdmClient } from "@typedecision/client";
import { createClient } from "@typedecision/client";

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

const tdmPlugin: Plugin = async () => {
  const judge = createJudgeHandler(getClient());
  return {
    tool: {
      tdm_judge: tool({
        description: TOOL_DESCRIPTION,
        args: judgeArgs,
        execute: (args, context) => judge(args, context),
      }),
    },
  };
};

export default tdmPlugin;
