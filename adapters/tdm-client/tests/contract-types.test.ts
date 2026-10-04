import type { DecisionRequest, Primitive } from "@typedecision/contract";
import { describe, expect, it } from "vitest";

// Type-level smoke: a request literal covering all three primitives must be
// accepted by the generated contract types (`satisfies` checked at compile
// time by `tsc --noEmit`; the test body guards the runtime shape).
const request = {
  state: { diff: "+ delete Database::flush()", env: "production" },
  questions: [
    {
      id: "pick-option",
      instructions: "Pick the rollback strategy with the least data loss.",
      primitive: {
        type: "choice",
        options: ["restore-snapshot", "replay-wal", "accept-loss"],
      },
    },
    {
      id: "confirm-proceed",
      instructions: "Is it safe to drop the staging table without a backup?",
      primitive: { type: "noul" },
    },
    {
      id: "blast-radius",
      instructions: "Rate the blast radius of this migration on the given scale.",
      primitive: { type: "score", levels: ["negligible", "minor", "major", "critical"] },
    },
  ],
} satisfies DecisionRequest;

describe("contract type smoke", () => {
  it("accepts a DecisionRequest literal covering all three primitives", () => {
    const kinds = request.questions.map((q) => q.primitive.type);
    expect(kinds).toEqual(["choice", "noul", "score"]);
  });

  it("narrows Primitive variants by their tag", () => {
    const primitives: Primitive[] = request.questions.map((q) => q.primitive);
    const choice = primitives[0];
    if (choice.type === "choice") {
      expect(choice.options).toHaveLength(3);
    } else {
      expect.unreachable("first question must be a choice");
    }
  });
});
