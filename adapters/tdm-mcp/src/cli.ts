#!/usr/bin/env node
/**
 * `tdm-mcp` bin entry: serve the three TDM tools over the MCP stdio transport.
 *
 * Bin pattern (deliberate): the bin points directly at this raw-TS file.
 * The source is written to be *type-stripping compatible* — no enums,
 * namespaces, or parameter properties, and relative imports carry explicit
 * `.ts` extensions — so it runs unmodified under:
 *   - Node >= 22.6 (`--experimental-strip-types`; unflagged since 23.6),
 *   - Bun (native TS),
 * plus any host that launches MCP servers via `npx`/`pnpm dlx`/`bunx`.
 * A thin JS wrapper was rejected: it would still end up importing raw TS,
 * so it only adds an indirection without removing the type-stripping
 * requirement.
 */

import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { createTdmServer } from "./server.ts";

const server = createTdmServer();

await server.connect(new StdioServerTransport());
