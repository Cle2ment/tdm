import { execSync } from "node:child_process";
import { existsSync } from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { beforeAll, describe, expect, it } from "vitest";

/**
 * Build-artifact test: self-building — if dist/index.js is missing (e.g. a
 * clean CI checkout), the package is built in beforeAll. Fully local — no
 * network access.
 *
 * - The existence assertion runs everywhere: after the build, dist/index.js
 *   must exist.
 * - The dynamic-import probe runs under bun only (the plugin host's other
 *   supported runtime); it asserts the bundle really evaluates to a V2
 *   plugin definition object.
 */
const packageDir = path.resolve(import.meta.dirname, "..");
const distEntry = path.join(packageDir, "dist/index.js");

const isBun = (process.versions as Record<string, string | undefined>).bun !== undefined;

describe("build artifact", () => {
  beforeAll(() => {
    if (!existsSync(distEntry)) {
      execSync("pnpm run build", { cwd: packageDir, stdio: "inherit" });
    }
  }, 120_000);

  it("dist/index.js exists (self-built when missing)", () => {
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
