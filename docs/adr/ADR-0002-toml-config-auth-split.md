# ADR-0002: TOML 配置 / config-auth 分离 / 分层解析

## Status

Accepted (2026-10-04)

## Context

TDM 同时供个人开发者（手工改配置、可 chezmoi/git 管理）与多 harness 会话使用，需要人可读、可
diff、可版本化的配置；密钥则绝不能随配置入库。plan §6 确定了文件布局与解析优先级（D4）。

## Decision

- 配置一律 TOML：`~/.config/tdm/config.toml`（全局，带 `version` 字段做前向迁移）、
  `<project>/.tdm/config.toml`（项目级覆盖，仅 config）。
- auth 独立成 `auth.toml`（`tdmm keys` 是推荐写入器而非唯一写入器），支持环境变量
  `TDM_<PROVIDER>_API_KEY` 覆盖；auth 永不项目级。
- 解析优先级：内置默认 → 全局 config → 项目 config → 环境变量 → 单次调用参数。

## Consequences

- 密钥泄漏面最小化；配置可审计、可迁移。
- 需要实现分层合并与校验逻辑（`tdmm config validate`），成本可控。
