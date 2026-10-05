/**
 * tdm-runtime platform loader.
 *
 * Resolves the native binding (`tdm-runtime.<triple>.node`) matching the
 * current platform/arch, in order:
 *   1. the published per-platform package `tdm-runtime-<triple>` (an
 *      optionalDependency, present for npm consumers), then
 *   2. a local binding beside this file, produced by
 *      `napi build --platform` (repo development, `pnpm build:debug`).
 *
 * TypeScript source, executed directly by the host (bun, or node >= 22.6 type
 * stripping); `napi build --no-js` guarantees the CLI never clobbers this
 * file with a generated JS loader.
 */

import { existsSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import type { DecisionRequest, DecisionResult, HealthReport } from "@typedecision/contract";

/** Harness + session identity attached to every call (audit/cache tagging). */
export interface SessionCtx {
  harness: string;
  sessionId: string;
}

/** Optional per-call overrides forwarded to the runtime. */
export interface JudgeOptions {
  provider?: string;
  session?: SessionCtx;
}

const BINARY_NAME = "tdm-runtime";

/** Platform triples this package ships bindings for (the release matrix). */
const SUPPORTED_TARGETS = [
  "win32-x64-msvc",
  "linux-x64-gnu",
  "linux-arm64-gnu",
  "darwin-x64",
  "darwin-arm64",
] as const;

/**
 * Detects musl (vs glibc) on Linux, so the right `.node` triple is picked.
 * Falls back to glibc when detection is unavailable (non-default Node builds).
 */
function isMusl(): boolean {
  if (process.platform !== "linux") return false;
  try {
    if (typeof process.report?.getReport === "function") {
      const report = process.report.getReport() as { header?: { glibcVersionRuntime?: string } };
      return report.header?.glibcVersionRuntime === undefined;
    }
  } catch {
    // Fall through to the glibc default.
  }
  return false;
}

/** The napi-rs platform triple for this process, or null if unsupported. */
function platformTriple(): string | null {
  const { platform, arch } = process;
  switch (platform) {
    case "win32":
      return arch === "x64" || arch === "arm64" ? `${platform}-${arch}-msvc` : null;
    case "darwin":
      return arch === "x64" || arch === "arm64" ? `${platform}-${arch}` : null;
    case "linux":
      return arch === "x64" || arch === "arm64"
        ? `linux-${arch}-${isMusl() ? "musl" : "gnu"}`
        : null;
    default:
      return null;
  }
}

interface NativeBinding {
  judge(req: unknown, opts?: unknown): Promise<unknown>;
  health(): Promise<unknown>;
}

function loadNativeBinding(): NativeBinding {
  const triple = platformTriple();
  if (triple === null) {
    throw new Error(
      `tdm-runtime does not support ${process.platform}-${process.arch}. ` +
        `Supported targets: ${SUPPORTED_TARGETS.join(", ")}.`,
    );
  }
  if (!(SUPPORTED_TARGETS as readonly string[]).includes(triple)) {
    const hint = triple.endsWith("-musl")
      ? ` Bindings are built against glibc only; musl targets (like ${triple}) are not shipped.`
      : "";
    throw new Error(
      `tdm-runtime ships no native binding for ${triple}.` +
        hint +
        ` Supported targets: ${SUPPORTED_TARGETS.join(", ")}.`,
    );
  }

  const require = createRequire(import.meta.url);

  // 1. Published consumers: the matching per-platform package is installed
  //    automatically as an optionalDependency of tdm-runtime.
  let requireError: unknown;
  try {
    return require(`tdm-runtime-${triple}`) as NativeBinding;
  } catch (error) {
    requireError = error; // Fall through to the local checkout binding.
  }

  // 2. Repo development: `pnpm --filter tdm-runtime build:debug` drops the
  //    binding beside this file.
  const bindingPath = join(
    dirname(fileURLToPath(import.meta.url)),
    `${BINARY_NAME}.${triple}.node`,
  );
  if (!existsSync(bindingPath)) {
    const cause = requireError instanceof Error ? requireError.message : String(requireError);
    throw new Error(
      `Missing tdm-runtime native binding for ${triple}. Tried:\n` +
        `  1. npm package "tdm-runtime-${triple}" (installed with tdm-runtime): ${cause}\n` +
        `  2. local file ${bindingPath} — build it from a tdm checkout with ` +
        `\`pnpm --filter tdm-runtime build\`.\n` +
        `Supported targets: ${SUPPORTED_TARGETS.join(", ")}.`,
    );
  }

  return require(bindingPath) as NativeBinding;
}

const nativeBinding = loadNativeBinding();

/**
 * Judges one batch of questions against the given state. Routed through the
 * Rust runtime (registry, capability check, exact-hash cache, retry/circuit,
 * audit). `opts.session` is wired into audit rows; `opts.provider` overrides
 * the configured default provider.
 */
export async function judge(req: DecisionRequest, opts?: JudgeOptions): Promise<DecisionResult> {
  return (await nativeBinding.judge(req, opts)) as DecisionResult;
}

/** Probes the provider that `judge` would route to by default. */
export async function health(): Promise<HealthReport> {
  return (await nativeBinding.health()) as HealthReport;
}
