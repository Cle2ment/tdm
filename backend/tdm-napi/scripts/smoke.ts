/**
 * Smoke test for @typedecision/runtime: loads the native binding and judges a
 * choice + noul + score batch through the deterministic mock provider (no
 * network, no API key). Run with node (`pnpm smoke`) or bun (`pnpm smoke:bun`).
 */

import type { DecisionRequest } from "@typedecision/contract";

import { health, judge } from "../index.ts";

function assert(condition: boolean, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

function assertCloseToOne(sum: number, label: string) {
  assert(Math.abs(sum - 1) < 1e-9, `${label} distribution must sum to ~1, got ${sum}`);
}

const CHOICE_OPTIONS = ["alpha", "beta", "gamma"];
const SCORE_LEVELS = ["low", "mid", "high"];

const request: DecisionRequest = {
  state: { repo: "tdm", filesChanged: 3 },
  questions: [
    {
      id: "choice-q",
      instructions: "pick one",
      primitive: { type: "choice", options: CHOICE_OPTIONS },
    },
    {
      id: "noul-q",
      instructions: "is it safe?",
      primitive: { type: "noul" },
    },
    {
      id: "score-q",
      instructions: "rate it",
      primitive: { type: "score", levels: SCORE_LEVELS },
    },
  ],
};

const ids = request.questions.map((question) => question.id);
const opts = { provider: "mock", session: { harness: "smoke", sessionId: "smoke-1" } };

try {
  const result = await judge(request, opts);

  assert(Array.isArray(result.answers), "answers must be an array");
  assert(
    JSON.stringify(result.answers.map((answer) => answer.id)) === JSON.stringify(ids),
    "answers must echo question ids in order",
  );
  console.log("PASS: answers echo ids in request order");

  const [choice, noul, score] = result.answers;

  assert(choice.value.type === "choice", `choice answer must be tagged "choice"`);
  assert(
    CHOICE_OPTIONS.includes(choice.value.type === "choice" ? choice.value.label : ""),
    "choice label must be one of the options",
  );
  if (choice.value.type === "choice") {
    assertCloseToOne(
      choice.value.distribution.reduce((sum, [, p]) => sum + p, 0),
      "choice",
    );
  }
  console.log("PASS: choice answer has the right type tag and sums to ~1");

  assert(noul.value.type === "noul", `noul answer must be tagged "noul"`);
  if (noul.value.type === "noul") {
    assert(
      noul.value.probability >= 0 && noul.value.probability <= 1,
      "noul probability must be in [0, 1]",
    );
  }
  console.log("PASS: noul answer has the right type tag");

  assert(score.value.type === "score", `score answer must be tagged "score"`);
  if (score.value.type === "score") {
    assert(SCORE_LEVELS.includes(score.value.level), "score level must be one of the levels");
    assert(typeof score.value.weighted === "number", "score weighted must be a number");
    assertCloseToOne(
      score.value.distribution.reduce((sum, [, p]) => sum + p, 0),
      "score",
    );
  }
  console.log("PASS: score answer has the right type tag and sums to ~1");

  assert(
    typeof result.usage.inputTokens === "number" && typeof result.usage.outputTokens === "number",
    "usage must carry numeric inputTokens/outputTokens",
  );
  console.log("PASS: usage is present");

  assert(result.provider.id === "mock", `provider.id must be "mock"`);
  console.log("PASS: provider.id === mock");

  const repeat = await judge(request, opts);
  assert(
    JSON.stringify(repeat.answers) === JSON.stringify(result.answers),
    "a second identical judge must return identical answers",
  );
  console.log("PASS: second identical judge returns identical answers (cache path exercised)");

  const report = await health();
  assert(report.ok === true, "mock provider health must be ok");
  console.log("PASS: health probe is ok");

  console.log(
    `SMOKE OK (${result.usage.inputTokens} input / ${result.usage.outputTokens} output tokens)`,
  );
} catch (error) {
  console.error(`SMOKE FAIL: ${error instanceof Error ? error.message : String(error)}`);
  process.exit(1);
}
