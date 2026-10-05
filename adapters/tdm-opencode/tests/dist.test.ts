import { existsSync } from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { describe, expect, it } from "vitest";

/**
 * Build-artifact test: run after `pnpm build` (the build script runs before
 * publish and is part of CI). Fully local — no network access.
 *
 * - The existence assertion runs everywhere: a missing dist/index.js means
 *   the package was not built.
 * - The dynamic-import probe runs under bun only (the plugin host's other
 *   supported runtime); it asserts the bundle really evaluates to a V2
 *   plugin definition object.
 */
const distEntry = path.resolve(import.meta.dirname, "../dist/index.js");

const isBun = (process.versions as Record<string, string | undefined>).bun !== undefined;

describe("build artifact", () => {
  it("dist/index.js exists (run `pnpm build` first)", () => {
    expect(existsSync(distEntry)).toBe(true);
  });

  it.runIf(isBun)("importing the bundle yields the tdm plugin definition (bun)", async () => {
    const mod = (await import(pathToFileURL(distEntry).href)) as {
      default: { id: string; setup: unknown };
    };
    expect(mod.default.id).toBe("tdm");
    expect(typeof mod.default.setup).toBe("function");
  });
});
