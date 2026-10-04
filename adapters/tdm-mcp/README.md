# @typedecision/mcp

TDM as an MCP server: exposes typed, auditable judgment to **any** MCP-capable
host over stdio (plan D5 — universal path + secrets barrier). The provider API
key lives inside this server process; agents only ever see three tools.

## Tools

| Tool            | Args                                                              | Returns                                   |
| --------------- | ----------------------------------------------------------------- | ----------------------------------------- |
| `tdm_judge`     | the `DecisionRequest` contract (`state`, `questions`) + optional `provider` / `session` overrides | the `DecisionResult` JSON (typed answers, distributions, usage) |
| `tdm_health`    | —                                                                 | provider `HealthReport` JSON              |
| `tdm_providers` | —                                                                 | provider ids the server can use + default |

Handlers never throw to the transport: client failures and argument-validation
errors both come back as `isError` text content carrying the message.

## Provider resolution (M1 approximation)

`TdmClient` has no provider-enumeration API yet, so `tdm_providers` reports the
env-resolved default: `TDM_JEV_API_KEY` or `TYPESAFE_API_KEY` present →
`jev`, otherwise `mock`. Full provider enumeration lands with the provider
registry.

## Running

```sh
npx @typedecision/mcp        # or: pnpm dlx / bunx
```

Then point any MCP host at the `tdm-mcp` command via stdio.

### Bin pattern (why the bin is a raw `.ts` file)

`bin` points directly at `src/cli.ts`. The source is written to be
type-stripping compatible — no enums, namespaces, or parameter properties, and
relative imports use explicit `.ts` extensions — so the same file runs
unmodified under:

- Node ≥ 22.6 (`--experimental-strip-types`; unflagged since 23.6),
- Bun (native TS).

A thin JS wrapper was rejected: it would still import raw TS, adding only
indirection without removing the type-stripping requirement.

## Development

```sh
pnpm --filter @typedecision/mcp test        # vitest (in-memory MCP transport, fake client)
pnpm --filter @typedecision/mcp typecheck   # tsc --noEmit
```
