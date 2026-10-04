# ADR-0001: Rust 核心 + napi-rs TS binding（含 Rpc 降级路径）

## Status

Accepted (2026-10-04)

## Context

TDM 的核心组件（契约类型、provider 适配、runtime 引擎、tdmm CLI）需要性能、类型安全与单二进制分
发；而接入方（opencode / pi / dsh / MCP）全部是 TypeScript 生态。需要一种跨语言桥接方式，且需预
留 Bun 等运行时的兼容性风险出口（plan §12）。

## Decision

- backend 全部用 Rust（edition 2024），跨语言唯一通道为两层：ts-rs 生成 TS 类型（契约单一事实
  源，D7）+ napi-rs 原生模块（`@typedecision/runtime`）。
- client 层先定义 `TdmClient` 抽象：默认 `NapiClient`（进程内），降级 `RpcClient`（`tdmm serve`
  的 newline-delimited JSON-RPC），实现可换。
- napi 矩阵构建从 M0 起进 CI，避免后期返工。

## Consequences

- adapters 永不直接触碰 Rust；native × Bun 的边缘问题由 RpcClient 兜底。
- 需维护生成物与 napi 构建链，工程复杂度略增；换来类型安全与性能。
