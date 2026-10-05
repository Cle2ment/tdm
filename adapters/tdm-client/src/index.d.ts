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
/**
 * Client backed by the in-process native binding (`tdm-runtime`,
 * built by napi-rs from `backend/tdm-napi`).
 *
 * The native module is imported lazily on the first judge/health call — never
 * at construction time — so clients can be created in environments where the
 * native module is absent until they actually call in.
 */
export declare class NapiClient implements TdmClient {
    #private;
    /** Module specifier the native binding is loaded from. */
    readonly runtimeModule: string;
    constructor({ runtimeModule }?: {
        runtimeModule?: string;
    });
    judge(req: DecisionRequest, opts?: JudgeOptions): Promise<DecisionResult>;
    health(): Promise<HealthReport>;
}
/**
 * Client that will talk JSON-RPC (newline-delimited, stdio) to the `tdmm
 * serve` daemon — the escape hatch for environments where the native binding
 * cannot load. Placeholder until `tdmm serve` lands in milestone M5, but the
 * interface exists so adapters can code against one surface today.
 */
export declare class RpcClient implements TdmClient {
    /** Command that will spawn the tdmm JSON-RPC daemon once implemented. */
    readonly command: string;
    constructor({ command }?: {
        command?: string;
    });
    judge(): Promise<DecisionResult>;
    health(): Promise<HealthReport>;
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
export declare function createClient({ mode }?: CreateClientOptions): TdmClient;
