import { defineConfig } from "tsdown";

/**
 * Emit the publishable, self-contained declaration entry (dist/index.d.ts):
 *
 * - `@typedecision/contract` is intentionally unpublished and bundled INLINE:
 *   its types resolve from the workspace TS sources and are copied into the
 *   emitted d.ts, so consumers typecheck without the package present. The
 *   import in index.ts is type-only and erased at runtime.
 * - Only dist/index.d.ts is emitted (`dts.emitDtsOnly`, no JS): `main` stays
 *   index.ts, executed directly by the host (bun, or node >= 22.6 type
 *   stripping).
 * - `generator: "tsc"` is required and pins this package to typescript "^6":
 *   rolldown-plugin-dts refuses the tsc generator on TS 7 (tsgo), same as
 *   adapters/tdm-opencode.
 */
export default defineConfig({
  entry: ["index.ts"],
  platform: "node",
  // fixedExtension defaults to true for platform node and forces .mjs —
  // disable it so the output is dist/index.d.ts (this package is "type":
  // "module").
  fixedExtension: false,
  dts: {
    tsconfig: "./tsconfig.build.json",
    generator: "tsc",
    eager: true,
    // Forwarded to rolldown-plugin-dts: emit only the .d.ts, no JS (main
    // stays index.ts, executed directly by the host).
    emitDtsOnly: true,
  },
});
