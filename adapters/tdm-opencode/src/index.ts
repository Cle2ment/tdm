/**
 * TDM harness adapter for opencode (M2): registers the `tdm_judge` tool AND
 * an automatic risk gate over every other tool execution.
 *
 * Plugin shape matches opencode v2 hosts: the module default-exports a
 * `Plugin` *definition object* — `{ id, setup }` — and the host invokes
 * `setup(ctx)` exactly once. `tdm_judge` is registered through
 * `ctx.tool.transform(editor => editor.add({...}))`, where tools take a
 * JSON Schema `input` (zod → `z.toJSONSchema`) and an async `execute`
 * returning `{ content }`. The host SDK (`@opencode/plugin`) is imported
 * type-only; `@typedecision/client` is bundled into this artifact and its
 * client is created lazily on first use, so plugin load stays side-effect
 * free. The client loads the native binding from `tdm-runtime` (the published
 * runtime package) by module name at call time.
 *
 * M2 risk gate — automatic scoring of tool executions:
 *
 * 1. `ctx.tool.hook("execute.before", ...)` fires for every tool call with
 *    `{ tool, sessionID, agent, messageID, id, input }`. The gate stashes a
 *    fire-and-forget TDM judgment keyed by the call id, skipping read-only
 *    allowlisted tools and `tdm_judge` itself.
 * 2. `ctx.permission.hook("evaluate", ...)` — every built-in and MCP tool
 *    routes its execution through the host's permission evaluation with
 *    `source: { type: "tool", messageID, id }`, and the host reads the
 *    post-hook `effect` back. When the policy escalates (low confidence,
 *    "dangerous" score, or high P(ask-first)), the gate mutates `effect` to
 *    `"ask"`, so opencode prompts the user before executing; rejecting the
 *    prompt blocks the call.
 *
 * Fail-open everywhere: if the judge is unavailable, calls are allowed with a
 * `console.warn`, and the gate never breaks host tool execution. The
 * confidence floor is overridable via the plugin's `confidenceFloor` option.
 */
import type { Plugin } from "@opencode/plugin";
import { NapiClient, type TdmClient } from "@typedecision/client";
import { z } from "zod";

import { judgeArgs } from "./args";
import { createRiskGate } from "./gate";
import { createJudgeHandler } from "./handler";

export { createRiskGate, type RiskGate } from "./gate";
export type { JudgeCallContext, JudgeHandler } from "./handler";
export { createJudgeHandler } from "./handler";

const TOOL_DESCRIPTION = `Run typed judgments through TDM for structured micro-decisions — routing, classification, verification, scoring — where calibrated, auditable answers beat free-form guessing.

Pass \`state\` (any JSON carrying the full decision context) plus one or more independent \`questions\`. Each question pairs self-contained \`instructions\` with one of three primitives:
- {"type":"choice","options":[...]} — pick exactly one option label;
- {"type":"noul"} — yes/no verification; the answer carries P(yes);
- {"type":"score","levels":[...]} — pick exactly one ordered level label.

Answers are typed and probabilistic: every answer carries a confidence, and choice/score answers include the full (label, probability) distribution. Prefer this tool over deciding inline whenever the judgment feeds code logic.`;

/**
 * Module specifier of the native binding the plugin loads at call time. The
 * published package depends on `tdm-runtime` (backend/tdm-napi), so the
 * client must load that name — not the workspace client's unpublished
 * `tdm-runtime` default. Constructing NapiClient directly (instead
 * of `createClient`) is what pins the specifier.
 */
const RUNTIME_MODULE = "tdm-runtime";

/**
 * Client singleton, created on the first tool call — mirrors NapiClient's lazy
 * native-module load and keeps plugin load free of side effects.
 */
let cachedClient: TdmClient | undefined;

function getClient(): TdmClient {
  cachedClient ??= new NapiClient({ runtimeModule: RUNTIME_MODULE });
  return cachedClient;
}

/** The DecisionRequest contract as a validating zod schema. */
const judgeRequest = z.object(judgeArgs);

/** LLM-facing tool input schema, derived once at module load. */
const judgeInputSchema = z.toJSONSchema(judgeRequest);

/** Anything registered during setup that must be disposed on cleanup. */
interface Disposable {
  dispose: () => Promise<void> | void;
}

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

    // M2 risk gate. Both hook domains are guarded: older/odd host builds may
    // not expose them (and the unit-test context passes a bare ctx), in which
    // case the gate degrades to "no automatic scoring" with a warning instead
    // of breaking plugin load.
    const disposables: Disposable[] = [registration];
    const gate = createRiskGate(getClient(), context.options);

    if (typeof context.permission?.hook === "function") {
      try {
        const evaluate = await context.permission.hook("evaluate", (event) =>
          gate.onPermissionEvaluate(event),
        );
        disposables.push(evaluate);
      } catch (cause) {
        console.warn(
          "[tdm] permission.evaluate hook unavailable, automatic risk scoring degraded:",
          cause instanceof Error ? cause.message : String(cause),
        );
      }
    }
    if (typeof context.tool.hook === "function") {
      try {
        const before = await context.tool.hook("execute.before", (event) =>
          gate.onExecuteBefore(event),
        );
        disposables.push(before);
      } catch (cause) {
        console.warn(
          "[tdm] tool.execute.before hook unavailable, automatic risk scoring degraded:",
          cause instanceof Error ? cause.message : String(cause),
        );
      }
    }

    return async () => {
      for (const disposable of disposables.reverse()) {
        try {
          await disposable.dispose();
        } catch (cause) {
          console.warn(
            "[tdm] dispose failed:",
            cause instanceof Error ? cause.message : String(cause),
          );
        }
      }
    };
  },
} satisfies Plugin.Plugin;
