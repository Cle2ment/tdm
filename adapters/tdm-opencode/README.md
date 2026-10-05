# @typedecision/opencode-tdm

[TDM (Typed Decision Model)](https://github.com/Cle2ment/tdm) plugin for [opencode](https://opencode.ai) — gives your agent a `tdm_judge` tool for typed, auditable micro-decisions, plus an automatic risk gate that escalates risky tool calls to user confirmation.

## Install

Add the package to the `plugins` array in your `opencode.jsonc` (note: the key is `plugins` — plural — in opencode V2):

```jsonc
{
  "plugins": ["@typedecision/opencode-tdm@latest"]
}
```

To pass options, use the object form:

```jsonc
{
  "plugins": [
    {
      "package": "@typedecision/opencode-tdm@latest",
      "options": {
        "confidenceFloor": 0.7
      }
    }
  ]
}
```

## What you get

### 1. The `tdm_judge` tool

A tool the agent can call for structured judgments instead of free-form guessing: routing, classification, yes/no verification (`noul`, with P(yes)), and ordered scoring. Every answer is typed and probabilistic — it carries a confidence, and choice/score answers include the full `(label, probability)` distribution, so decisions can feed code logic and stay auditable.

### 2. Automatic risk gate

The plugin hooks tool execution (`execute.before`) and permission evaluation (`permission.evaluate`). Every judgment-worthy tool call — anything not a read-only allowlist tool (read/grep/glob/webfetch/…) and not `tdm_judge` itself — is scored by TDM along two axes: how risky is executing without asking, and should the agent ask first. When the verdict escalates (score level "dangerous", confidence below the floor, or P(ask-first) ≥ 0.8), the plugin mutates the permission effect to `"ask"`, so opencode prompts you before executing; rejecting the prompt blocks the call.

The gate is strictly fail-open: judge outages or uncorrelated events never break host tool execution (at worst a `console.warn`).

## Requirements

- Node.js >= 22.6 or Bun.
- The native runtime binding `@typedecision/runtime` is installed automatically as a dependency of this package.
- A judge provider key: set `TYPESAFE_API_KEY` or `TDM_JEV_API_KEY` in the environment for real model-backed judgments.
- **Without a key**, the runtime degrades to a deterministic **mock provider**: the tool and the risk gate keep working (schema, plumbing, escalation paths), but the answers are canned, not intelligent. Do not rely on mock-provider verdicts for safety decisions.

## Options

| Option            | Type     | Default | Description                                                                                   |
| ----------------- | -------- | ------- | --------------------------------------------------------------------------------------------- |
| `confidenceFloor` | `number` | `0.55`  | Minimum score-answer confidence for a tool call to pass the gate unasked; below → ask user. |

## Links

- Repository: <https://github.com/Cle2ment/tdm> (this package lives in `adapters/tdm-opencode`)
- Security policy: <https://github.com/Cle2ment/tdm/blob/main/SECURITY.md>

## License

Apache-2.0
