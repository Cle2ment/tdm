import { describe, expect, it } from "vitest";
import { createClient, NapiClient, RpcClient } from "../src/index";

describe("createClient", () => {
  it("returns NapiClient for mode 'napi'", () => {
    expect(createClient({ mode: "napi" })).toBeInstanceOf(NapiClient);
  });

  it("returns NapiClient for mode 'auto'", () => {
    expect(createClient({ mode: "auto" })).toBeInstanceOf(NapiClient);
  });

  it("returns NapiClient when mode is omitted (auto default)", () => {
    expect(createClient()).toBeInstanceOf(NapiClient);
  });

  it("returns RpcClient for mode 'rpc'", () => {
    expect(createClient({ mode: "rpc" })).toBeInstanceOf(RpcClient);
  });
});
