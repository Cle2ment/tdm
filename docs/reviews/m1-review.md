# M1 Review — runtime / tdmm / napi / mcp / conformance

> 2026-10-05 · 审查人：orchestrator（oracle 因配额不可用，由编排方直读源码执行）
> 范围：backend/tdm-runtime, backend/tdmm, backend/tdm-napi, backend/conformance, adapters/tdm-mcp
> 结论：**通过，可在其上构建 M2**。无 CRITICAL。

## SHOULD-FIX（进入 M2 待办）

1. **stats() 成本统计虚高** — `store.rs` `StatsRow` 的 token 求和包含 cache 命中行（命中行记录了原始 usage）。缓存命中不消耗 API token，成本统计应只计未命中：`SUM(CASE WHEN cached=0 THEN input_tokens ELSE 0 END)`。相关：成本核算是项目目标之一，此口径会误导。
2. **缓存键不含线上模型版本** — `lib.rs` 缓存键用的是配置期 model（"jev-latest"），而 wire 回传真实版本（"jev-1.13.0"）。服务端静默升级模型时旧缓存不失效。建议：cache 表加 `model` 列、命中时校验；配套需要 `tdmm cache clear`（当前不存在）。
3. **`max_state_bytes` 未实施** — capability 协商只检查 primitives，从不校验 state 大小。judge() 路由后应加一道字节数检查。
4. **缓存键规范化依赖 serde_json 默认 BTreeMap 序** — 若未来任何依赖开启 `preserve_order` feature（workspace 级 feature 统一），缓存键将变为插入序敏感。现有测试 `cache_key_ignores_json_object_order` 是绊线，保留即可，注释已如实声明。

## NIT

5. 熔断半开探针若挂起（provider 无超时路径），`probe_in_flight` 永久卡位；实践中被 provider 10s 超时兜住。可接受。
6. 审计 `request_json` 存全量 state——设计如此，但需在文档明示：**state 不得含机密**（adapter 侧责任，judged 前先脱敏）。
7. `tdmm logs` 时间戳为原始 unix 秒（`--json` 有完整值）。M1 可接受。

## 审查中确认的强项

- judge 管道顺序正确（路由 → capability → 缓存 → 熔断 → 重试 → 审计），所有失败路径均落审计。
- 熔断半开探针闩锁正确防惊群；Mutex 临界区无 await。
- 密钥脱敏三层覆盖（TdmConfig Debug 只列键名、JevConfig Debug 手写 redacted、tdmm keys list 前 4 字符）。
- 认证解析链完整：env → auth.toml → legacy TYPESAFE_API_KEY（jev 限定）。
- 缓存读写/审计失败只 warn 不阻断判定——运行面韧性优先，查询面如实报错，取舍正确。

## M2 开工前提

无阻塞项。SHOULD-FIX 1–3 建议在 M2 顺手带上（均为小改动）。
