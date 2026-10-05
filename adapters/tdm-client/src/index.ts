import type { DecisionRequest, DecisionResult, HealthReport } from "@typedecision/contract";

export type { DecisionRequest, DecisionResult, HealthReport } from "@typedecision/contract";

/**
 * Harness + session identity attached to every call, so the runtime can tag
 * audit rows, cache entries, and cost stats per harness session.
 */
export interface SessionCtx {
  harness: string;
  sessionId: string;
}

/** Optional per-call overrides forwarded to the runtime. */
export interface JudgeOptions {
  session?: SessionCtx;
  provider?: string;
}

/**
 * Single adapter-facing surface for TDM judgments. Adapters code against this
 * interface and receive a concrete transport via {@link createClient}.
 */
export interface TdmClient {
  judge(req: DecisionRequest, opts?: JudgeOptions): Promise<DecisionResult>;
  health(): Promise<HealthReport>;
}

/** Shape of the `tdm-runtime` napi-rs binding module. */
type NapiRuntime = {
  judge(req: DecisionRequest, opts?: JudgeOptions): Promise<DecisionResult>;
  health(): Promise<HealthReport>;
};

const DEFAULT_RUNTIME_MODULE = "tdm-runtime";

/**
 * Client backed by the in-process native binding (`tdm-runtime`,
 * built by napi-rs from `backend/tdm-napi`).
 *
 * The native module is imported lazily on the first judge/health call — never
 * at construction time — so clients can be created in environments where the
 * native module is absent until they actually call in.
 */
export class NapiClient implements TdmClient {
  /** Module specifier the native binding is loaded from. */
  readonly runtimeModule: string;

  #runtime: NapiRuntime | undefined;

  constructor({ runtimeModule }: { runtimeModule?: string } = {}) {
    this.runtimeModule = runtimeModule ?? DEFAULT_RUNTIME_MODULE;
  }

  async judge(req: DecisionRequest, opts?: JudgeOptions): Promise<DecisionResult> {
    const runtime = await this.#loadRuntime();
    return runtime.judge(req, opts);
  }

  async health(): Promise<HealthReport> {
    const runtime = await this.#loadRuntime();
    return runtime.health();
  }

  async #loadRuntime(): Promise<NapiRuntime> {
    if (this.#runtime !== undefined) return this.#runtime;

    let mod: unknown;
    try {
      mod = await import(this.runtimeModule);
    } catch (cause) {
      throw new Error(
        `Failed to load the TDM native runtime from "${this.runtimeModule}". tdm-runtime is the native (napi-rs) binding package — install it with \`pnpm add tdm-runtime\`, or build it from a tdm checkout with \`cargo build -p tdm-napi --release\` (see backend/tdm-napi).`,
        { cause },
      );
    }

    const runtime = resolveRuntime(mod);
    if (runtime === undefined) {
      throw new Error(
        `Module "${this.runtimeModule}" was loaded but does not look like the tdm-runtime native binding (expected judge/health exports). Reinstall the package or rebuild it with \`cargo build -p tdm-napi --release\`.`,
      );
    }
    this.#runtime = runtime;
    return runtime;
  }
}

/**
 * Accept either named exports (`mod.judge`) or a single default export
 * (CommonJS interop of the napi module).
 */
function resolveRuntime(mod: unknown): NapiRuntime | undefined {
  const named = mod as Partial<NapiRuntime> | null | undefined;
  if (typeof named?.judge === "function" && typeof named.health === "function") {
    return named as NapiRuntime;
  }
  const def = (mod as { default?: unknown } | null | undefined)?.default as
    | Partial<NapiRuntime>
    | null
    | undefined;
  if (typeof def?.judge === "function" && typeof def.health === "function") {
    return def as NapiRuntime;
  }
  return undefined;
}

/**
 * Client that will talk JSON-RPC (newline-delimited, stdio) to the `tdmm
 * serve` daemon — the escape hatch for environments where the native binding
 * cannot load. Placeholder until `tdmm serve` lands in milestone M5, but the
 * interface exists so adapters can code against one surface today.
 */
export class RpcClient implements TdmClient {
  /** Command that will spawn the tdmm JSON-RPC daemon once implemented. */
  readonly command: string;

  constructor({ command }: { command?: string } = {}) {
    this.command = command ?? "tdmm serve";
  }

  async judge(): Promise<DecisionResult> {
    throw new Error("RpcClient is not implemented yet (lands with tdmm serve, milestone M5)");
  }

  async health(): Promise<HealthReport> {
    throw new Error("RpcClient is not implemented yet (lands with tdmm serve, milestone M5)");
  }
}

export interface CreateClientOptions {
  /**
   * - `"napi"` (default): in-process native binding.
   * - `"rpc"`: JSON-RPC over stdio via `tdmm serve` (unimplemented until M5).
   * - `"auto"`: currently resolves to napi; once RpcClient lands it will
   *   prefer napi and fall back to rpc where native loading is unavailable.
   */
  mode?: "auto" | "napi" | "rpc";
}

/** Build a {@link TdmClient} for the requested transport. */
export function createClient({ mode }: CreateClientOptions = {}): TdmClient {
  if (mode === "rpc") return new RpcClient();
  return new NapiClient();
}
