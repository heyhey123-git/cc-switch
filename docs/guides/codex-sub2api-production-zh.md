# Codex + CC Switch + Sub2API：Claude 接入与运维

核对日期：2026-11-21。验证组合：CC Switch v4.0.31 本分支、
Codex 0.171.0-alpha.13、Windows。其他版本先按本文验收，再用于正式工作。
本文和示例均不包含账户密钥、个人数据库或安装包。

## 1. 这份修复解决了什么

| 项目 | 当前结果 |
| --- | --- |
| Fable 5.2、Opus 5.5、Sonnet 5.5、Haiku 5.5 | 保留全部入口，五档推理原样转换，默认 medium |
| xhigh | 支持的型号保留 xhigh；旧型号保留原有 max 回退 |
| Haiku 5.5 | 使用 adaptive thinking，不再发送旧式 budget_tokens |
| Adaptive 工具续接 | 模型未输出思考块时仍允许回传合法工具结果；已有签名块原样保留 |
| 图片、普通函数工具、签名回放 | 本地协议检查及真实 Codex 客户端流程通过 |
| 压缩与压缩后继续调用工具 | 本地流程通过；未据此宣称百万 token 实网压缩性能已验证 |
| 窗口访问 localhost:3000 | 已用包含前端资源的生产构建修复并实际打开窗口验证 |
| 本次部署的鉴权、真实缓存、图片、工具续接、摘要后继续 | 实网小额验收通过；其他部署按第 7 节验收 |

本分支已通过 92 项原生回归测试。模拟缓存计数只能证明用量转换，
不能证明付费上游真的缓存了输入。原生 Codex 也可以命中缓存。
本次 Sonnet 5.5 / medium 小额实网验收已确认缓存、图片、普通工具续接和摘要后继续；
没有完成百万 token 压缩性能验收。

## 2. 获取并构建修复版

克隆个人 fork 的修复分支：

```powershell
git clone --branch fix/claude-effort-haiku-55 https://github.com/heyhey123-git/cc-switch.git
cd cc-switch
pnpm install --frozen-lockfile
pnpm build:renderer
cd src-tauri
cargo +stable build --release --locked --features tauri/custom-protocol
```

仓库使用 pnpm 10.12.3。Windows 需要 Rust、Microsoft C++ Build Tools 和 WebView2；
依赖准备按仓库开发文档执行。已验证构建使用 Rust stable 1.100.0。
以上是已验证的生产可执行文件构建路径；如需平台安装包，可使用仓库的 `pnpm build`。
产物为 `src-tauri/target/release/cc-switch.exe`。已缓存所有 Rust 依赖时可加
`--offline`。单独运行默认 `cargo build --release` 不足以验证桌面生产打包。
启用代理不代表 UI 已验收；必须实际打开窗口，确认没有开发服务器连接错误。
替换安装文件前退出 CC Switch，并保留当前安装包及配置备份。

## 3. 配置链路

```text
Codex --Responses / HTTP SSE--> CC Switch 127.0.0.1:15721
      --Anthropic Messages--> Sub2API /v1/messages --> Claude
```

在 CC Switch 的 **Codex** 页签添加自定义供应商：

| 字段 | 设置 |
| --- | --- |
| 名称 | 自行命名，例如 Claude Gateway |
| API 请求地址 | 商家的根地址，例如 https://gateway.example.com；不要重复拼接 /v1/messages |
| API Key | 在本机填写自己的有效密钥 |
| 上游格式 | Anthropic Messages |
| 认证字段 | 按商家要求；Sub2API 通常选择 ANTHROPIC_AUTH_TOKEN / Authorization |
| 模拟 Claude Code | 关闭 |
| 最大输出 tokens | 16384 |
| 默认模型 | claude-sonnet-5-5 |
| 默认推理档位 | medium |

然后开启“路由总开关”和“Codex 路由”，启用这个供应商。
Codex 的生效地址应为 `http://127.0.0.1:15721/v1`，协议仍是
`wire_api = "responses"`；商家地址保留在 CC Switch 的上游供应商配置中。
不要把 Messages 地址直接当成 Codex 的 Responses 地址。

以下是 CC Switch 的上游供应商配置示例，**不是接管后直接覆盖 Codex 主配置的文件**。
替换示例地址，在本机供应商凭据中填写自己的 Key。

```toml
# CC Switch upstream provider template, not the live Codex proxy configuration.
# Replace the example URL; enter the real key in CC Switch's credential field.
model_provider = "claude_gateway"
model = "claude-sonnet-5-5"
model_reasoning_effort = "medium"
model_context_window = 1000000
model_auto_compact_token_limit = 800000
model_auto_compact_token_limit_scope = "body_after_prefix"
tool_output_token_limit = 12000

[model_providers.claude_gateway]
name = "Claude Gateway"
base_url = "https://gateway.example.com/v1"
env_key = "CLAUDE_GATEWAY_API_KEY"
wire_api = "responses"
requires_openai_auth = false
supports_websockets = false
request_max_retries = 0
stream_max_retries = 0
stream_idle_timeout_ms = 300000

[model_providers.claude_gateway.capabilities]
remote_compaction = "unsupported"
```

对应元数据字段如下；这是字段片段，不是整库备份或一键导入文件。

```json
{
  "commonConfigEnabled": false,
  "apiFormat": "anthropic",
  "apiKeyField": "ANTHROPIC_AUTH_TOKEN",
  "impersonateClaudeCode": false,
  "maxOutputTokens": 16384
}
```

关闭本地路由的自动故障转移。本文采用 HTTP SSE，
将 provider 的 `supports_websockets` 设为 false，
`request_max_retries` 和 `stream_max_retries` 设为 0，
`stream_idle_timeout_ms` 设为 300000。
请求出错后由使用者决定是否重试；该配置不提供自动恢复。
如果多个模型共享这个 provider，传输及重试设置也会共同生效。

## 4. 模型目录、上下文与图片

官方资料在核对日列出以下四个型号均支持图片、1M 上下文、
最大 128K 输出，以及 low / medium / high / xhigh / max 五档推理。
中转站必须实际开放对应模型与容量；填写目录不会增加服务端配额。

| 模型 ID | 建议默认档位 | 本方案上下文 | 本方案单次输出上限 |
| --- | --- | --- | --- |
| claude-fable-5-2 | medium | 1000000 | 16384 |
| claude-opus-5-5 | medium | 1000000 | 16384 |
| claude-sonnet-5-5 | medium | 1000000 | 16384 |
| claude-haiku-5-5 | medium | 1000000 | 16384 |

在供应商模型映射中逐项填写模型 ID、上下文窗口和五档推理。
下面以 Sonnet 为例，字段属于供应商 `settingsConfig.modelCatalog`，不是 Codex 原始 catalog。
其余三个模型使用上表对应的模型 ID，分别添加同样的窗口、模态和推理档位设置。
已有供应商仅合并这一字段，不覆盖 auth、config 或其他模型。

```json
{
  "modelCatalog": {
    "models": [
      {
        "model": "claude-sonnet-5-5",
        "displayName": "Claude Sonnet 5.5",
        "contextWindow": 1000000,
        "inputModalities": ["text", "image"],
        "supportsParallelToolCalls": false,
        "reasoningLevels": ["low", "medium", "high", "xhigh", "max"],
        "defaultReasoningLevel": "medium"
      }
    ]
  }
}
```
CC Switch 生成的实际 catalog 至少核对：

- `input_modalities = ["text", "image"]`。
- `context_window` 和 `max_context_window` 为 1000000。
- `default_reasoning_level = "medium"`，五个 `supported_reasoning_levels` 正确。
- `supports_image_detail_original = false`；使用普通图片质量即可。

只使用此供应商的配置可设 `model_auto_compact_token_limit = 800000` 和
`model_auto_compact_token_limit_scope = "body_after_prefix"`。
混合多个供应商的聚合目录应把上下文/压缩限制放进对应 Claude 条目：
`context_window = 1000000`、`auto_compact_token_limit = 800000`；
不要用全局 1M 配置覆盖所有其他型号。重新生成目录后复核这些值。
客户端保留的有效窗口可能小于标称值，本次显示约 950000。

设置 `[model_providers.<id>.capabilities]` 下的
`remote_compaction = "unsupported"`，让 Codex 使用其本地编排的压缩流程。
这仍可能调用模型生成摘要并计费，不意味着离线或免费压缩。
`body_after_prefix` 控制门槛计数范围，不代表删除历史或免除输入费用。

完全退出并重新启动 Codex，再新建同一模型的对话。不要把旧协议、其他模型或损坏的
半截工具历史混入新会话。Adaptive thinking 有时不输出思考块，这不属于签名损坏，
合法的工具结果可以直接继续回传；若产生了签名块，则需原样保留。
实际附加 PNG/JPEG/WebP 等有效图片；
仅发送本机文件路径不等于上传图片。让模型描述图中一个明确细节作为看图验收。

## 5. 为什么以前缓存少，正确请求是什么

本次旧链路记录表现为大量历史重复计入输入、主要复用一小段固定前缀。
原生 Codex 已发送稳定的 `prompt_cache_key`，因此不能把原因归为“没有使用
CC Switch”。该字段与 Anthropic 的 `cache_control` 是不同协议机制。
本方案通过 Messages 转换，让实际发送的系统、工具和消息包含缓存断点，
并把上游缓存读写统计转换回 Codex。

下面只展示正确字段形态，短示例本身不足以触发最低缓存长度：

```http
POST /v1/messages
Authorization: Bearer <YOUR_API_KEY>
anthropic-version: 2023-06-01
Content-Type: application/json
```

```json
{
  "model": "claude-sonnet-5-5",
  "max_tokens": 16384,
  "thinking": { "type": "adaptive" },
  "output_config": { "effort": "medium" },
  "system": [
    {
      "type": "text",
      "text": "<固定系统提示和需要复用的长前缀>",
      "cache_control": { "type": "ephemeral" }
    }
  ],
  "messages": [
    { "role": "user", "content": "本轮问题" }
  ]
}
```

这些字段由 CC Switch 转换，日常使用者不必手动构造 JSON。
缓存按相同前缀复用；模型、工具定义、图片、系统提示、thinking 设置或前缀历史改变时，
命中范围可能改变。保持同一模型和会话，在缓存有效期内继续；不要后台反复预热。
本地默认标记是 5 分钟，最多四个断点。Sub2API 的实际版本可能重写断点、
TTL 或账号路由，需要结合商家的请求日志确认最终发给上游的内容。

查看 `cache_read_input_tokens`、`cache_creation_input_tokens`；
Codex 对应 `cachedInputTokens`、`cacheWriteInputTokens`。
Codex 总输入包含缓存部分，不应将总输入全部按新鲜输入单价计算。
1 小时缓存写入与 5 分钟写入价格不同，本分支的用量转换没有完整保留两种 TTL 的细分。
本地成本看板是估算，最终与商家扣量对账。

## 6. 额度查询与费用设置

在供应商“更多 → 配置用量查询”中开启查询。Sub2API 使用 `GET /v1/usage`，
密钥从供应商配置取，不复制到脚本中。

- 只有余额/总额度时，可先选内置 Sub2API 模板。
- 需要显示 `rate_limits` 中 5h/7d 多窗口时，选自定义，粘贴下方脚本。
- 超时设 15 秒，自动查询间隔设 5 分钟；先手动运行一次核对字段与单位。
- total、used、remaining、reset_at 从接口读取，不把某个账户的额度写死给其他人。

```javascript
// Paste this expression into CC Switch's custom usage-query editor.
// Credentials are substituted by CC Switch; do not hard-code them here.
({
  request: {
    url:
      "{{baseUrl}}".replace(/\/+$/, "").replace(/\/v1$/, "") + "/v1/usage",
    method: "GET",
    headers: {
      Authorization: "Bearer {{apiKey}}",
      "User-Agent": "cc-switch/1.0",
    },
  },
  extractor: function (response) {
    var invalid = function (message) {
      return { isValid: false, invalidMessage: String(message) };
    };
    var finite = function (value) {
      return typeof value === "number" && isFinite(value) && value >= 0;
    };
    if (!response || typeof response !== "object") {
      return invalid("额度接口未返回对象");
    }
    if (response.isValid === false || response.error) {
      return invalid(response.status || response.message || "额度查询失败");
    }
    var quota = response.quota || {};
    var unit = response.unit || quota.unit || "额度";
    if (Array.isArray(response.rate_limits) && response.rate_limits.length) {
      var windows = response.rate_limits.map(function (window) {
        if (!window || typeof window !== "object") {
          return invalid("额度窗口格式不正确");
        }
        if (
          !finite(window.limit) &&
          !finite(window.used) &&
          !finite(window.remaining)
        ) {
          return invalid("额度窗口缺少有效数值");
        }
        var names = { "5h": "5小时额度", "7d": "7天额度" };
        var result = {
          isValid: true,
          planName: names[window.window] || String(window.window || "自定义") + "额度",
          unit: unit,
          extra: "重置：" + String(window.reset_at || "服务端未提供"),
        };
        if (finite(window.limit)) result.total = window.limit;
        if (finite(window.used)) result.used = window.used;
        if (finite(window.remaining)) result.remaining = window.remaining;
        return result;
      });
      return windows;
    }
    var result = { isValid: true, unit: unit };
    if (typeof response.planName === "string") result.planName = response.planName;
    if (finite(response.remaining)) result.remaining = response.remaining;
    if (finite(quota.limit)) result.total = quota.limit;
    if (finite(quota.used)) result.used = quota.used;
    if (
      result.remaining === undefined &&
      result.total === undefined &&
      result.used === undefined
    ) {
      return invalid("额度接口格式不匹配，请按商家实际响应调整提取器");
    }
    return result;
  },
})
```

示例提取器兼容多窗口和普通余额响应；遇到未知格式报错，不显示虚构余额。
接口未提供计费单位时显示“额度”，不要擅自当成美元。
5h/7d 是滚动窗口，不等于“每日/每月美元限额”；不要互相代填。
价格表按自己商家的当前输入、输出、缓存读写单价填写。
本地看板与查询脚本负责显示；需要硬性停止扣费时配置商家服务端额度上限。

## 7. 正式使用前的最小验收

本地协议测试不访问付费模型。下面两条短请求属于真实模型调用，
**只有使用者确认费用后才手动执行**；不要拿 Fable 或百万 token 历史做接入测试。

1. 先只查询一次额度。遇到 401，处理 Key 有效性；403 检查响应来源、认证方式和出口；
   429 检查窗口额度。不要连续重试模型请求。
2. 固定一个 2K～4K token 的文本前缀，同一 Sonnet 模型、同一会话和档位，
   输出限制为 32 token，串行发送两次，间隔在缓存有效期内。
   两次请求共用相同前缀。为这个独立验收供应商暂设 `maxOutputTokens=32`，
   验收后恢复 16384；该元数据会覆盖请求体自己的输出限制。
3. 核对第二次响应的缓存读取是否覆盖前次大部分固定前缀，
   并对照前后额度差额。仅看到请求含缓存标记不能记为命中。
4. 再附加一张小图片，完成一次短工具调用与手动压缩；
   核对压缩后实际请求历史已改变，且下一轮能继续调用工具。
5. 任一步有错误、扣量异常或没有预期命中就停下保留时间、request ID 和去密钥的用量；
   不通过扩大上下文或并发调用尝试修复缓存。

通过鉴权、真实缓存、图片、工具和压缩这几项后，才将该部署记录为“生产验收通过”。
本分支没有替任何新账户执行上述付费验收。

## 8. 已知限制与常见故障

| 现象/能力 | 处理 |
| --- | --- |
| 窗口提示 localhost 拒绝连接 | 确认使用包含前端的生产构建；无需为正式使用启动 3000 开发服务器 |
| 本机 15721 拒绝连接 | 检查 CC Switch 正在运行、路由与 Codex 接管开关已打开 |
| thinking/signature 400 | 保持同模型签名历史完整；已损坏的旧历史新建会话 |
| 只缓存很小的固定前缀 | 查看实际 Messages 断点、TTL、会话路由和回传统计 |
| 流中断、输出突然结束 | 核对内部 status/incomplete_details；不能只看 response.completed 事件名 |
| strict JSON Schema | 当前转换会丢弃对应请求字段，不按严格结构化输出能力使用 |
| Codex 内置 web_search | Anthropic 路径不提供该原生工具，需要另行配置受支持的搜索工具 |
| 原生图片 detail=original | 不做等价保证；使用普通图片输入 |
| 压缩时 CPU 或内存异常 | 本次没有完成百万 token 性能验收；保留进程、输入规模与日志后单独分析 |

不要用反复大请求诊断认证、缓存和压缩问题。公开问题报告只上传去密钥日志，
不上传 `auth.json`、CC Switch 数据库或含凭据的备份。

## 9. 更新与回归

官方安装包更新可能覆盖本地修复，界面仍显示 4.0.31 也不能证明二进制相同。
记录当前构建提交及文件 SHA256；在新版源码上检查这两个文件并迁移仍需要的补丁：

- `src-tauri/src/proxy/providers/transform_codex_anthropic.rs`
- `src-tauri/src/proxy/thinking_optimizer.rs`

原生回归命令（不调用模型；测试目录使用一次性路径）：

```powershell
cd src-tauri
$env:CC_SWITCH_TEST_HOME = Join-Path ([IO.Path]::GetTempPath()) ("cc-switch-tests-" + [guid]::NewGuid())
cargo +stable test --locked --lib proxy::providers::transform_codex_anthropic::tests -- --test-threads=1
cargo +stable test --locked --lib proxy::thinking_optimizer::tests -- --test-threads=1
Remove-Item Env:CC_SWITCH_TEST_HOME
```

两个命令必须都成功。跨版本时重新测试实际桌面 UI、四模型五档、图片、签名回放和
压缩流程；不要将旧可执行文件覆盖到新版安装目录来“保留补丁”。
数据库恢复只处理明确需要的配置，勿用旧整库覆盖后续新数据。

## 参考资料

- [Anthropic 模型概览](https://platform.claude.com/docs/en/about-claude/models/overview)
- [Anthropic Effort](https://platform.claude.com/docs/en/build-with-claude/effort)
- [Anthropic Adaptive Thinking](https://platform.claude.com/docs/en/build-with-claude/adaptive-thinking)
- [Anthropic Prompt Caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)
- [Anthropic Vision](https://platform.claude.com/docs/en/build-with-claude/vision)
- [Codex 配置参考](https://developers.openai.com/codex/config-reference/)
- [OpenAI Prompt Caching](https://developers.openai.com/api/docs/guides/prompt-caching)
- [本仓库基础路由说明](codex-claude-routing-guide-zh.md)

本文优先采用本分支实际验证结果。例如旧路由说明中的输出默认值、完全无损及
自动命中表述，不能替代本版本对输出上限、JSON Schema 和真实缓存的逐项验收。
