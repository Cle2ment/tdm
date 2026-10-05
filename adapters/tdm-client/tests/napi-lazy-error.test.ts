import { beforeEach, describe, expect, it, vi } from "vitest";
import { NapiClient } from "../src/index";

// Tracks how often the native runtime module is actually imported. The mock
// factory only runs on import, so the counter doubles as a load probe.
const state = vi.hoisted(() => ({ loadAttempts: 0 }));

vi.mock("tdm-runtime", () => {
  state.loadAttempts += 1;
  throw new Error("synthetic module load failure");
});

const REQUEST = { state: {}, questions: [] };

describe("NapiClient lazy loading", () => {
  beforeEach(() => {
    state.loadAttempts = 0;
  });

  it("does not import the runtime module at construction time", () => {
    new NapiClient();
    expect(state.loadAttempts).toBe(0);
  });

  it("rejects judge() with a helpful install/build message when the module cannot be loaded", async () => {
    const client = new NapiClient();
    await expect(client.judge(REQUEST)).rejects.toThrow(
      /@typedecision\/runtime is the native \(napi-rs\) binding package/,
    );
    expect(state.loadAttempts).toBeGreaterThan(0);
  });

  it("rejects health() with the same guidance", async () => {
    const client = new NapiClient();
    await expect(client.health()).rejects.toThrow(/pnpm add @typedecision\/runtime/);
  });
});
