import type { DecisionRequest, TdmClient } from "@typedecision/client/src/index";

/**
 * Minimal slice of the opencode `ToolContext` the judge handler consumes. The
 * SDK's ToolContext structurally satisfies this, so the plugin passes its
 * context straight through while tests inject a bare object.
 */
export interface JudgeCallContext {
  sessionID?: string;
}

/** Execute-style handler: DecisionRequest in, tool output string out. */
export type JudgeHandler = (args: DecisionRequest, context?: JudgeCallContext) => Promise<string>;

/**
 * Build the `tdm_judge` execute handler against an injected client. Pure —
 * tests pass a fake {@link TdmClient}; the plugin passes the lazily created
 * real one.
 *
 * Never throws: client / native-module failures surface as a descriptive
 * error string, so a broken TDM install cannot crash the plugin host.
 */
export function createJudgeHandler(client: TdmClient): JudgeHandler {
  return async (args, context = {}) => {
    try {
      const result = await client.judge(args, {
        session: {
          harness: "opencode",
          sessionId: context.sessionID ?? "unknown",
        },
      });
      return JSON.stringify(result, null, 2);
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause);
      return `TDM judge failed: ${message}`;
    }
  };
}
