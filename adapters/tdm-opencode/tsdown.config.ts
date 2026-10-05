import { defineConfig } from "tsdown";

/**
 * Build the publishable plugin artifact:
 *
 * - `@typedecision/client` and `@typedecision/contract` are bundled INLINE
 *   (devDependencies: tiny, unpublished sources resolved from the workspace).
 * - `zod` and `@opencode/plugin` stay EXTERNAL so the plugin shares the host's
 *   single zod instance and SDK types (see the zod pin in package.json).
 * - `@typedecision/runtime` stays a real runtime dependency (the native napi
 *   binding); the bundled client loads it lazily via a dynamic import by
 *   module name, so there is nothing to inline or resolve at build time.
 */
export default defineConfig({
  entry: ["src/index.ts"],
  format: "esm",
  platform: "node",
  // Emit .js/.d.ts (not .mjs/.d.mts) to match the package.json entry points.
  // tsdown's fixedExtension defaults to true for platform node and forces
  // .mjs — disable it since this package is "type": "module".
  fixedExtension: false,
  // `generator: "tsc"` is required: TypeScript 7 (tsgo) emits declarations
  // with `--rootDir <tsconfig dir>`, which cannot cover the workspace sources
  // bundled from `../tdm-client` and `../tdm-contract`; the tsc generator
  // emits per-module via the TS API. It also requires typescript < 7 — the
  // package pins "^6" for exactly this reason.
  dts: { tsconfig: "./tsconfig.build.json", generator: "tsc", eager: true },
  external: ["zod", "@opencode/plugin", "@typedecision/runtime"],
});
