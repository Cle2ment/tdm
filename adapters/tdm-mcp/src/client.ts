/**
 * Client acquisition + provider-id resolution for the tdm-mcp server (D5).
 *
 * The TdmClient lives entirely inside this server process: it holds whatever
 * provider credentials the environment carries, and MCP clients only ever see
 * the three tools — never the key (the secrets barrier).
 *
 * M1 approximation for `tdm_providers`: the TdmClient surface has no
 * provider-enumeration API yet, so the tool reports the *default* provider id
 * resolved from the environment (see {@link resolveDefaultProviderId}):
 * `TDM_JEV_API_KEY` / `TYPESAFE_API_KEY` present → "jev", otherwise "mock".
 * Full provider enumeration lands with the provider registry.
 */

import { createClient, type TdmClient } from "@typedecision/client";

export interface ProviderResolution {
  /** The provider id calls default to. */
  default: string;
  /** Ids the client can currently see (M1: exactly one — the default). */
  providers: Array<string>;
  /** How the id was resolved. */
  source: "env";
}

/** Env-var names checked, in order, for the jev provider key. */
const JEV_KEY_VARS = ["TDM_JEV_API_KEY", "TYPESAFE_API_KEY"] as const;

/**
 * Resolve the default provider id from environment rules:
 * `TDM_JEV_API_KEY` or `TYPESAFE_API_KEY` present → "jev", else "mock".
 */
export function resolveDefaultProviderId(
  env: Record<string, string | undefined> = process.env,
): string {
  return JEV_KEY_VARS.some((name) => (env[name] ?? "").length > 0) ? "jev" : "mock";
}

/** Summarize what the client can see, per the M1 env-resolution approximation. */
export function resolveProviderInfo(
  env: Record<string, string | undefined> = process.env,
): ProviderResolution {
  const provider = resolveDefaultProviderId(env);
  return { default: provider, providers: [provider], source: "env" };
}

export interface LazyClient {
  /** Create the client on first use, then reuse it (mirrors NapiClient's own lazy native load). */
  get(): TdmClient;
}

/** Lazily create the auto-transport client on first tool call. */
export function createLazyClient({
  create = createClient,
}: {
  create?: () => TdmClient;
} = {}): LazyClient {
  let client: TdmClient | undefined;
  return {
    get() {
      client ??= create();
      return client;
    },
  };
}
