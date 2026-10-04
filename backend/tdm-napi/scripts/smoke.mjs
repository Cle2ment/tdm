/**
 * Smoke test for @typedecision/runtime: loads the native binding and judges a
 * choice + noul + score batch through the deterministic mock provider (no
 * network, no API key). Run with node (`pnpm smoke`) or bun (`pnpm smoke:bun`).
 */

import * as runtimeNs from "../index.js";

/**
 * Accepts either named exports or a single default export (CJS interop
 * differences between node and bun).
 */
function resolveRuntime(ns) {
  if (typeof ns.judge === "function" && typeof ns.health === "function") return ns;
  const def = ns.default;
  if (typeof def?.judge === "function" && typeof def?.health === "function") return def;
  throw new Error("native binding does not expose judge/health exports");
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function assertCloseToOne(sum, label) {
  assert(Math.abs(sum - 1) < 1e-9, `${label} distribution must sum to ~1, got ${sum}`);
}

const runtime = resolveRuntime(runtimeNs);

const request = {
  state: { repo: "tdm", filesChanged: 3 },
  questions: [
    {
      id: "choice-q",
      instructions: "pick one",
      primitive: { type: "choice", options: ["alpha", "beta", "gamma"] },
    },
    {
      id: "noul-q",
      instructions: "is it safe?",
      primitive: { type: "noul" },
    },
    {
      id: "score-q",
      instructions: "rate it",
      primitive: { type: "score", levels: ["low", "mid", "high"] },
    },
  ],
};

const ids = request.questions.map((question) => question.id);

try {
  const result = await runtime.judge(request, { provider: "mock" });

  assert(Array.isArray(result.answers), "answers must be an array");
  assert(
    JSON.stringify(result.answers.map((answer) => answer.id)) === JSON.stringify(ids),
    "answers must echo question ids in order",
  );
  console.log("PASS: answers echo ids in request order");

  const [choice, noul, score] = result.answers;

  assert(choice.value.type === "choice", `choice answer must be tagged "choice"`);
  assert(
    request.questions[0].primitive.options.includes(choice.value.label),
    `choice label ${choice.value.label} must be one of the options`,
  );
  assertCloseToOne(
    choice.value.distribution.reduce((sum, [, p]) => sum + p, 0),
    "choice",
  );
  console.log("PASS: choice answer has the right type tag and sums to ~1");

  assert(noul.value.type === "noul", `noul answer must be tagged "noul"`);
  assert(
    typeof noul.value.probability === "number" &&
      noul.value.probability >= 0 &&
      noul.value.probability <= 1,
    "noul probability must be a number in [0, 1]",
  );
  console.log("PASS: noul answer has the right type tag");

  assert(score.value.type === "score", `score answer must be tagged "score"`);
  assert(
    request.questions[2].primitive.levels.includes(score.value.level),
    `score level ${score.value.level} must be one of the levels`,
  );
  assert(typeof score.value.weighted === "number", "score weighted must be a number");
  assertCloseToOne(
    score.value.distribution.reduce((sum, [, p]) => sum + p, 0),
    "score",
  );
  console.log("PASS: score answer has the right type tag and sums to ~1");

  assert(
    result.usage &&
      typeof result.usage.inputTokens === "number" &&
      typeof result.usage.outputTokens === "number",
    "usage must carry numeric inputTokens/outputTokens",
  );
  console.log("PASS: usage is present");

  assert(result.provider && result.provider.id === "mock", `provider.id must be "mock"`);
  console.log("PASS: provider.id === mock");

  const health = await runtime.health();
  assert(health.ok === true, "mock provider health must be ok");
  console.log("PASS: health probe is ok");

  console.log(
    `SMOKE OK (${result.usage.inputTokens} input / ${result.usage.outputTokens} output tokens)`,
  );
} catch (error) {
  console.error(`SMOKE FAIL: ${error.message}`);
  process.exit(1);
}
