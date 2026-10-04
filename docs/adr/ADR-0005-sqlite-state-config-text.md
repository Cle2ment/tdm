# ADR-0005: SQLite 只存状态、配置走文本文件

## Status

Accepted (2026-10-04)

## Context

runtime 需要持久化三类数据：审计（每次判断一行）、缓存（exact-hash）、统计（成本/延迟）。这些都
是"机器写、机器读"的状态数据；而配置是"人读人写、需 diff 与评审"的数据。两者形态不同，不应混在
一种存储里。

## Decision

- SQLite（rusqlite，bundled）只存状态：审计库 `~/.local/share/tdm/audit.db`、缓存、统计聚合。
- 所有配置走 TOML 文本文件（见 ADR-0002）；tdmm 是推荐写入器，但文本可被任意工具（chezmoi、
  编辑器、脚本）读写，程序不假定独占。

## Consequences

- 状态可查询可聚合（`tdmm logs` / `tdmm stats` 直接 SQL）；配置可评审可版本化。
- 需维护一个轻量 schema 迁移；SQLite 文件不入版本库（.gitignore 已排除生成物）。
