# ADR-0004: 插件 hook 主路径 vs MCP 通用路径（密钥屏障）

## Status

Accepted (2026-10-04)

## Context

四个 harness 适配插件都具备事件/hook 机制，可以在动作执行前拦截；MCP 则是任何客户端都能挂的通用
通道。但 MCP server 进程若持有密钥，密钥面就扩大到每个客户端会话（plan D5）。

## Decision

- hook 强制拦截为主路径（如 tdm-opencode 的 `tool.execute.before`），低置信度才升级为人工询问；
  插件通过 `@typedecision/client` 调 TDM。
- tdm-mcp 保留为通用补充路径，只注册 judge / health / stats 三个精简工具；**密钥屏障**：MCP 进
  程内不持有 provider 密钥，判断经本机 napi/RPC 通道完成，密钥只存在于 tdm 侧。

## Consequences

- 常规操作对用户无感，低置信度才打断；密钥不出 TDM 进程边界。
- 两套路径都要维护审计来源标记（hook vs 工具），统计口径需区分。
