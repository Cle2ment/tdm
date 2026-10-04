# ADR-0003: Provider 语义接口 + capability 协商与降级

## Status

Accepted (2026-10-04)

## Context

Provider 从 jev（TypeSafe System One）起步，后续接入 StartLux-Decision 等；各家的 API 形态差异
大，但语义能力可归约为三个 primitive（choice / noul / score）。路由需要一张能力表做协商，而非对
每个 provider 写特判。

## Decision

- `tdm-core` 定义 `DecisionProvider` trait：`id` / `capabilities` / `judge` / `health`，接口按
  语义能力中立建模，不暴露任何 provider 私有概念。
- `Capabilities { primitives, batch, max_state_bytes }` 驱动路由协商：请求所需 primitive 不满足
  默认拒绝（`TdmError::Unsupported`）；`score→多 noul 转译`作为 config 开关，默认关。
- 错误统一为 `TdmError` 分类（Network/Server/Client/Quality/Unsupported），`is_retryable()`
  驱动重试/熔断/升级策略。

## Consequences

- provider 可替换性有根本保障（conformance 套件双绿）；新增 provider 成本为"实现 trait + 过
  conformance"。
- 语义中立意味着 provider 特有能力只能进 provider 配置，不能进接口。
