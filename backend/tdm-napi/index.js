/**
 * @typedecision/runtime platform loader.
 *
 * Loads the prebuilt native binding (`tdm-runtime.<triple>.node`) matching the
 * current Node platform/arch. Without an napi publish pipeline (per-platform
 * packages), only local bindings produced by `napi build --platform` are
 * supported — see the error message below for how to build one.
 */

const { existsSync } = require("node:fs");
const { join } = require("node:path");

const BINARY_NAME = "tdm-runtime";

/** Platform triples this package ships bindings for. */
const SUPPORTED_TARGETS = [
  "win32-x64-msvc",
  "win32-arm64-msvc",
  "darwin-x64",
  "darwin-arm64",
  "linux-x64-gnu",
  "linux-x64-musl",
  "linux-arm64-gnu",
  "linux-arm64-musl",
];

/**
 * Detects musl (vs glibc) on Linux, so the right `.node` triple is picked.
 * Falls back to glibc when detection is unavailable (non-default Node builds).
 */
function isMusl() {
  if (process.platform !== "linux") return false;
  try {
    if (typeof process.report?.getReport === "function") {
      const report = process.report.getReport();
      return report.header?.glibcVersionRuntime === undefined;
    }
  } catch {
    // Fall through to the glibc default.
  }
  return false;
}

/** The napi-rs platform triple for this process, or null if unsupported. */
function platformTriple() {
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

function loadNativeBinding() {
  const triple = platformTriple();
  if (triple === null) {
    throw new Error(
      `@typedecision/runtime does not support ${process.platform}-${process.arch}. ` +
        `Supported targets: ${SUPPORTED_TARGETS.join(", ")}.`,
    );
  }

  const bindingPath = join(__dirname, `${BINARY_NAME}.${triple}.node`);
  if (!existsSync(bindingPath)) {
    throw new Error(
      `Missing @typedecision/runtime native binding for ${triple} (expected ${bindingPath}). ` +
        `Supported targets: ${SUPPORTED_TARGETS.join(", ")}. ` +
        "Build it from a tdm checkout with `pnpm --filter @typedecision/runtime build`.",
    );
  }

  return require(bindingPath);
}

const nativeBinding = loadNativeBinding();

// Assigned per-property (not `module.exports = ...`) so node's cjs-module-lexer
// can statically detect the named exports for ESM `import { judge }` consumers.
module.exports.judge = nativeBinding.judge;
module.exports.health = nativeBinding.health;
