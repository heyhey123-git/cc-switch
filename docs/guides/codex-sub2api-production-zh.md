# Codex + CC Switch + Sub2API：Claude 接入与运维

核对日期：2026-11-22。验证组合：CC Switch v4.0.38 本分支、
Codex 0.171.0-alpha.13 / Windows，以及 Codex 0.160.0 / Ubuntu 24.04 x86_64 服务端。
其他版本先按本文验收，再用于正式工作。
本文和示例均不包含账户密钥、个人数据库或安装包。

## 1. 这份修复解决了什么

| 项目 | 当前结果 |
| --- | --- |
| Fable 5.2、Opus 5.5、Sonnet 5.5、Haiku 5.5 | 保留全部入口，五档推理原样转换，默认 medium |
| xhigh | 支持的型号保留 xhigh；旧型号保留原有 max 回退 |
| Haiku 5.5 | 使用 adaptive thinking，不再发送旧式 budget_tokens |
| Adaptive 工具续接 | 模型未输出思考块时仍允许回传合法工具结果；已有签名块原样保留 |
| 工具顶层 oneOf / anyOf / allOf / $ref | 将完整约束置于对象属性内；返回参数和历史回放自动还原，兼容流式及命名空间工具 |
| 工具参数中的 _id / _meta / __typename | 采用上游已包含的修复，保留原始工具参数 |
| OpenAI 工具 strict 字段 | 采用上游修复，不将该字段透传给 Anthropic / Bedrock 工具定义 |
| 图片、普通函数工具、签名回放 | 本地协议检查及真实 Codex 客户端流程通过 |
| 图片缩放通知 | 修复版 4 同时覆盖上传图片和工具图片、多图及分段通知，保留历史原位；普通 developer 指令仍进入 system |
| 工具顺序和长尾追加 | Claude 工具定义按名称稳定排序；同一会话前缀未变时，保留上次请求的消息断点，避免追加超过 20 块后找不到旧断点 |
| 缓存排查证据 | 输出工具、system、设置和历史前缀是否变化的指纹日志；不记录正文、图片、思考文本或密钥 |
| Linux SSH 工作端 | `--headless` 在服务器本机提供代理，独立于 Windows，不需要图形桌面 |
| 压缩与压缩后继续调用工具 | 本地流程通过；未据此宣称百万 token 实网压缩性能已验证 |
| 窗口访问 localhost:3000 | 已用包含前端资源的生产构建修复并实际打开窗口验证 |
| 压缩的 SSE 响应 | 在事件转换前增量解压 gzip、deflate（zlib）、br、zstd 及堆叠编码；不等待整条流结束 |
| 私有 CA 站点 | 可在本机按 HTTPS origin 固定证书 SHA256；仍检查域名、有效期及 TLS 握手签名，拒绝重定向 |
| 鉴权、真实缓存、图片、工具续接、摘要后继续 | 旧修复版曾通过 Sonnet 小额实网验收；本次升级只执行不计费的本地回归，其他部署按第 7 节验收 |

Windows 客户端验收使用独立 Codex 配置目录和本地模拟上游，覆盖四模型五档、25 个工具、517828 字符合成上下文、图片、压缩与压缩后继续。
修复版 4 额外核对上传图、工具图、工具顺序及长尾追加的实际转换前缀；原生回归及安装产物验收结果见对应发行页。
模拟缓存计数只能证明用量转换，
不能证明付费上游真的缓存了输入。原生 Codex 也可以命中缓存。
此前 Sonnet 5.5 / medium 小额实网验收已确认缓存、图片、普通工具续接和摘要后继续；
没有完成百万 token 压缩性能验收。

## 2. 获取并构建修复版

### Windows 直接安装

下载 [Windows x64 安装包（MSI）](https://github.com/heyhey123-git/cc-switch/releases/download/v4.0.38-claude-fix.4/CC-Switch-4.0.38-Claude-Fix.4-Windows-x64.msi)，
双击后按安装向导完成即可，不需要自己编译。适用于 Windows 10/11 x64；
缺少 WebView2 时安装器会联网下载运行时。更新已有安装前先退出 CC Switch。

这是个人 fork 的修复版，程序内版本显示 4.0.38，未使用官方签名证书。
安装包不包含 API Key、账户数据或供应商配置；安装后从第 3 节开始配置。
安装官方新版可能覆盖本补丁，更新方式见第 9 节。

### 从源码构建

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

### Linux：供 SSH 远程 Codex 工作端使用

SSH 协作时，请求由服务器上的 Codex app-server 发起，因此服务器要有自己的 CCS 进程、
配置和密钥。`127.0.0.1:15721` 指服务器本机。这个部署不依赖 Windows CCS 或端口转发。
已验证的目标是 Ubuntu 24.04 x86_64、Python 3.12；配置脚本要求 Python 3.11 或更高版本。

Linux 首轮二进制已在 Ubuntu 24.04 无桌面环境完成本地模拟验收。
包含修复版 4 的 Linux 安装产物正在独立构建验收，发布后从
[发行页](https://github.com/heyhey123-git/cc-switch/releases)下载 `.deb`、
`cc-switch-proxy@.service`、`configure-linux-proxy.py` 和 `SHA256SUMS`。
以下是对应部署流程，不能把 Windows MSI 当作 Linux 安装包。
下载后先核对 SHA256，再安装；二进制仍链接 GTK/WebKit 运行库，但 headless 模式不初始化图形界面，
无需桌面、DISPLAY 或 Xvfb。桌面版和 headless 服务不要同时占用同一用户的配置及端口。

```bash
sudo apt install ./CC-Switch-4.0.38-Claude-Fix.4-Linux-x64.deb
sudo install -m 644 cc-switch-proxy@.service /etc/systemd/system/
```

下面的初始化和配置命令应由**运行远程 Codex 的同一用户**执行。
先确认这个用户已经有可工作的 API Key 型 Codex 配置，且 `model_catalog_json`
指向现有模型目录；脚本会导入该供应商，保留默认模型、原有模型、项目、功能配置及 `auth.json`。
OAuth 登录配置不适用这个导入脚本。

```bash
cc-switch --headless --init-only
umask 077
mkdir -p ~/.cc-switch
# 用编辑器创建下面示例对应的私密配置文件。
${EDITOR:-vi} ~/.cc-switch/linux-provider.json
python3 configure-linux-proxy.py --input ~/.cc-switch/linux-provider.json --dry-run
python3 configure-linux-proxy.py --input ~/.cc-switch/linux-provider.json
sudo systemctl daemon-reload
sudo systemctl enable --now "cc-switch-proxy@$(id -un)"
systemctl status "cc-switch-proxy@$(id -un)" --no-pager
curl --noproxy '*' http://127.0.0.1:15721/health
```

私密输入文件示例，填入自己的信息，不要提交到 Git：

```json
{
  "namespace": "claude",
  "anthropic": {
    "base_url": "https://YOUR-RELAY.example/v1",
    "api_key": "YOUR-PRIVATE-KEY"
  }
}
```

应用前会在 `~/.cc-switch/backups/before-linux-proxy-*` 备份待修改文件和数据库；
重复执行配置脚本会拒绝覆盖已有接入。脚本另加 `cc_switch_remote` 供应商，使用独立本地令牌，
不替换原来的 `auth.json`。原 GPT 入口默认继续可选，四个 Claude 入口为
`ccs-claude/claude-fable-5-2`、`ccs-claude/claude-opus-5-5`、
`ccs-claude/claude-sonnet-5-5`、`ccs-claude/claude-haiku-5-5`。
可在私密输入中提供与第 4 节一致的 `claude_models` 数组，以使用已经验收的模型元数据。

私有 CA 的站点仍需按第 3 节配置服务器自己的 `~/.cc-switch/tls-pins.json`，
使用经可信渠道确认的证书指纹，不关闭 TLS 校验。
CCS 的出站代理留空，网络规则在服务器自己的 Clash/mihomo 中配置。
headless 服务提供代理功能，额度页面使用中转站面板；它不提供桌面配置窗口。

现有远程 Codex 进程不强制重启。准备好后重新连接 SSH 工作端，
让新启动的 app-server 读取服务器配置，再在模型菜单选择 Claude。
排查服务使用 `journalctl -u "cc-switch-proxy@$(id -un)" -n 80 --no-pager`。
更新 Linux 包时先等待服务没有在处理模型请求，再重启该服务。

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
request_max_retries = 3
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

CC Switch 的全局出站代理留空；需要选择出口、规则或 TUN 时在 Clash 中设置。
Clash 必须绕过 `127.0.0.0/8`、`localhost` 和 `::1`，使 Codex 到 CCS 的本机连接直接可达。
避免再在 CCS 内叠加 `http://127.0.0.1:7897` 等代理地址。

关闭本地路由的自动故障转移。本文采用 HTTP SSE，
将 provider 的 `supports_websockets` 设为 false，
`request_max_retries` 设为 3，`stream_max_retries` 保持 0，
`stream_idle_timeout_ms` 设为 300000。
请求阶段遇到 HTTP 502 等 5xx 时，由 Codex 自动退避重试，最多重试 3 次（连同首次共 4 次）；
达到上限仍失败就停止。这个原生设置也适用于其他符合条件的请求传输错误，不限于 502。
HTTP 400 不会因此重试；已经开始输出后的 SSE 中断不自动重放，保留用户判断是否继续的机会。
这能缓解短暂故障导致的中断，不会消除服务端持续返回 502 的原因，也不保证失败请求未在上游计费。
本地实际 Codex 客户端检查通过：两次 502 后第三次成功、持续 502 仅发 4 次、400 仅发 1 次、
已经输出部分内容的中断仅发 1 次；全部连接本地模拟服务，未调用付费模型。
已有配置要在当前 Codex `config.toml` 及 CC Switch 对应供应商保存的 TOML 中同步修改，
避免切换供应商后恢复成 0。更新后重启 Codex，确保现有会话也采用新配置；CC Switch 无需为此重启。
如果多个模型共享这个 provider，传输及重试设置也会共同生效。

### 私有 CA 的 HTTPS 站点

有些自建站使用私有 CA，浏览器能打开不代表 CC Switch 信任其证书。
优先由站点配置公开可信证书，或通过可信渠道取得真实根 CA。
当站点只能提供私有证书、使用者明确接受首次连接的信任风险时，本修复版支持固定叶证书指纹。
它仅替代该站的 CA 链验证；证书指纹、主机名、有效期和 TLS 握手签名仍须通过检查。

在 CC Switch 配置目录创建 `tls-pins.json`。Windows 默认位置为
`%USERPROFILE%\.cc-switch\tls-pins.json`；使用自定义数据目录时放在同一数据库目录下。
内容是 HTTPS origin 到证书 SHA256 的 JSON 对象：

```json
{
  "https://gateway.example.com:21137": "替换为64位十六进制证书SHA256"
}
```

键只包含协议、域名和端口，不包含 `/v1`、路径或末尾斜杠。
证书指纹由站点管理员通过可信渠道提供最佳；若自行首次获取，必须清楚这次获取尚未经过 CA 验证。
指纹是完整 DER 叶证书的 SHA256，不是公钥指纹，也不是 SHA1。
请将示例值替换为实际 64 位十六进制指纹后保存；推荐 UTF-8 无 BOM。
这个文件不含 API Key，不随安装包分发，不导入 Windows 根证书库。

如需查看当前证书，可在 PowerShell 中执行以下只建立 TLS、不提交 API Key 的代码。
`$relayHost` 与 `$relayPort` 填写自己的站点；检查输出后再将指纹写入文件。

```powershell
$relayHost = "gateway.example.com"
$relayPort = 21137
$tcp = [Net.Sockets.TcpClient]::new($relayHost, $relayPort)
$tls = [Net.Security.SslStream]::new($tcp.GetStream(), $false, { $true })
try {
    $tls.AuthenticateAsClient($relayHost)
    $cert = [Security.Cryptography.X509Certificates.X509Certificate2]::new($tls.RemoteCertificate)
    $sha = [Security.Cryptography.SHA256]::Create()
    try {
        $fingerprint = ([BitConverter]::ToString($sha.ComputeHash($cert.RawData))).Replace("-", "").ToLowerInvariant()
    } finally { $sha.Dispose() }
    $cert | Format-List Subject, Issuer, NotBefore, NotAfter
    "SHA256: $fingerprint"
} finally { $tls.Dispose(); $tcp.Dispose() }
```

配置后先手动查询一次额度，确认成功，再开始工作。新请求会重新读取指纹文件，
连接池按 origin、指纹和出站代理分别复用；其他域名、子域名和端口继续使用正常 CA 验证。
固定证书的请求禁止 HTTP 重定向，避免凭据被转发到未核验入口。
证书更新后旧指纹会拒绝连接，取得新证书并核对后更新本机文件。
本方案不能代替服务端自动续期证书；分发 MSI 时让每位使用者配置自己的站点与凭据。

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
本修复会改变含顶层组合约束的工具定义，升级后的首轮可能重新写入这部分前缀；
保持工具集稳定后再观察后续缓存读取，不用大上下文反复做升级验收。
本地默认标记是 5 分钟，最多四个断点。Sub2API 的实际版本可能重写断点、
TTL 或账号路由，需要结合商家的请求日志确认最终发给上游的内容。

修复版 2 会把 Codex 看图后生成的 `image_resize_notice` developer 条目提升到全局 system。
每次新增尺寸提示都会改变消息之前的前缀，长会话可能只命中工具等较短的固定部分。
修复版 3 仅将完整匹配 Codex 固定格式的图片尺寸元数据作为对应工具结果中的文字保留在原位；
没有相邻工具结果时在原历史位置保留，不跨越其他内容回填到旧工具结果。
尺寸信息和图片均保留，不删除提示，也不将普通 developer 指令降为用户内容。
本地回归覆盖连续两次看图、超过 50 万字符的历史、缓存标记和混合 developer 指令。
修复版 3 的匹配范围仅包含 `preceding tool output`，遗漏了用户上传图产生的
`preceding user message`。修复版 4 同时识别这两种来源、多张图、CRLF 及多个纯通知文本块。
通知只含固定格式的数字尺寸数据时才按历史元数据处理；夹带普通指令、图片或未知内容的
developer 条目继续保留原指令权限。

修复版 4 还对转换后的 Claude 工具数组按名称排序，避免相同工具仅因枚举顺序不同而造成全量重建。
同一供应商、同一会话中，如果工具、system、设置和历史前缀未变，备用断点优先保留上次请求的
消息断点，覆盖一次追加超过 20 个内容块的情况；仍最多四个断点，默认 5 分钟，不增加预热请求。
HTTP 502、发送失败或成功响应校验失败时，不把该次尝试的新断点当作已写入位置，
保留之前已接受请求的断点，供后续继续或重试使用。HTTP 200 仅表示响应已接受，
真实缓存写入仍以返回用量和账单为准。
跟踪保存在内存中，最多 128 个会话、每会话前 8192 块；进程重启或超出跟踪范围时使用常规断点策略。

日志中的 `[CACHE_PREFIX]` 会显示 `tools_changed`、`system_changed`、`settings_changed`、
`shared_blocks` 和前后块数，便于对照同一时刻的缓存账单。日志只包含摘要指纹和数量，
不保存提示正文、工具参数、图片、思考内容或密钥。`initial=true` 表示本进程尚无前一请求可对比。
它证明本地前缀是否变化，不代表服务端确认缓存写入成功。

16,384 是本方案配置的单次总输出上限，包含 thinking，不是模型的上下文上限。
xhigh 可能将这部分额度全部用在思考上，出现 `max_output_tokens` / incomplete；
自动目标续接会再发下一轮。不要把这种相邻现象直接认定为缓存失效原因，
应同时核对前缀日志和用量。提高输出上限也会提高单次费用风险，不为排查缓存自动提高。
已有会话中旧尺寸提示的位置在升级时会改变，首轮可能重新写入大段缓存；
先用短会话核对新版本，不要主动重放长历史或反复预热。修复本地前缀变化不代表每次请求一定命中远端缓存。

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
迁移网关时保留实际模型 ID，例如 `claude-fable-5-2`，不要为了兼容把它映射成 Opus。
模型列表只证明入口存在；额度 GET 不会产生新的 Fable 消费记录，不能据此证明其扣费单价。
需要核对价格时查看站点后台的模型定价、分组倍率和已有账单，不通过大请求试扣费。
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
| input_schema 顶层 oneOf / allOf / anyOf 400 | 安装本修复版；保留原始 schema，不手工删除约束或工具 |
| 502：connection closed / TLS EOF / 连接被重置 | 查看供应商和实际出站错误；CCS 出站代理留空，在 Clash 核对 TUN、DNS、规则及节点状态。本机 /responses 是客户端入口 |
| 只缓存很小的固定前缀 | 查看实际 Messages 断点、TTL、会话路由和回传统计 |
| 看图后大段缓存重写 | 使用修复版 4，覆盖用户上传图与工具图通知；检查 `[CACHE_PREFIX]`，升级旧会话时注意一次性重建 |
| 跨轮续接突然完全不命中 | 对齐工具、system、设置及 shared_blocks 变化；相同工具仅顺序改变和长尾追加已有本地回归，不能仅凭输出达到上限断言原因 |
| upstream_status: HTTP 502 且没有错误详情 | 记录时间与请求 ID，对照站内 /v1/messages 错误日志；不要与本地 Connect / SendRequest 错误混为一类。图片缓存修复不代表此类 502 已解决 |
| 流中断、输出突然结束 | 核对内部 status/incomplete_details；不能只看 response.completed 事件名 |
| gzip SSE 出现乱码或没有终止事件 | 使用修复版 2；压缩字节须先解码再交给事件改写器，单纯删掉 Content-Encoding 会损坏响应 |
| 私有 CA / unknown issuer / certificate verify failed | 按第 3 节配置可信 CA 或本机证书指纹；证书更新后核对新指纹 |
| strict JSON Schema | 当前转换会丢弃对应请求字段，不按严格结构化输出能力使用 |
| Codex 内置 web_search | Anthropic 路径不提供该原生工具，需要另行配置受支持的搜索工具 |
| 原生图片 detail=original | 不做等价保证；使用普通图片输入 |
| 压缩时 CPU 或内存异常 | 本次没有完成百万 token 性能验收；保留进程、输入规模与日志后单独分析 |

不要用反复大请求诊断认证、缓存和压缩问题。公开问题报告只上传去密钥日志，
不上传 `auth.json`、CC Switch 数据库或含凭据的备份。

CC Switch 的全局出站代理会同时影响 GPT 和 Claude，本方案将它留空。
检查 502 时先记录错误时间，区分 Connect 建连超时、SendRequest 发送时断连和 SSE 流中断，
再对齐 Clash 的内核日志与连接记录。重启恢复能说明链路状态发生改变，但不能单独确定
具体是节点、TUN、DNS 还是连接状态异常。短请求正常不能代替长请求或 SSE 验证。
早前的 Claude 工具和推理补丁只改转换层。本次修复版 2 增加了 Responses 分派前的
流式解压，以及显式固定证书站点的客户端选择；未配置该站指纹时继续使用原有客户端。
CCS 出站代理保持原设置，不用修改连接池重试参数处理证书错误。
请求发送后断连时上游可能已经开始处理，不用提高自动重试次数掩盖错误。

一次现场调查确认过 Clash 界面期望 TUN 开启，但实际内核未启用；仅切换开关后又出现
169.254 地址且缺少默认路由。重新加载完整配置后恢复预期虚拟网卡地址、路由和 DNS。
因此重启内核恢复时，应同时核对实际内核配置及网卡路由，而不只看界面的开关。
另一个出口曾在 TLS 握手阶段返回 EOF，调整该目标的 Clash 规则后恢复。
这些现场结果与本地复现的 gzip SSE 问题分别处理；没有证据把所有断流归于同一原因。

顶层组合 schema 兼容层只作用于 Codex 到 Anthropic 的工具转换。
它将原始 schema 放在必填 `_ccs_input` 属性中，返回时拆除这一层，历史回放时再添加；
本地 JSON pointer 引用随之调整，普通工具和属性内部的组合约束保持原样。
非法包装参数会返回失败，不会把包装字段传给实际工具执行。

## 9. 更新与回归

官方安装包更新可能覆盖本地修复，界面显示相同版本也不能证明二进制相同。
需要保留本补丁时关闭自动安装官方更新，从个人 fork 的后续修复发行版更新。
记录当前构建提交及文件 SHA256；在新版源码上检查以下文件并迁移仍需要的补丁：

- `src-tauri/src/proxy/providers/transform_codex_anthropic.rs`
- `src-tauri/src/proxy/providers/streaming_codex_anthropic.rs`
- `src-tauri/src/proxy/thinking_optimizer.rs`
- `src-tauri/src/proxy/cache_injector.rs`、`cache_diagnostics.rs` 及 `forwarder.rs` 的 Claude 专用注入调用
- Linux 的 `headless_proxy.rs`、入口、systemd 单元和 `scripts/configure-linux-proxy.py`
- `src-tauri/src/proxy/content_encoding.rs` 和 `handlers.rs` 中的 SSE 解码入口
- `src-tauri/src/proxy/tls_pins.rs`、`forwarder/send.rs`、`http_client.rs` 和 `src-tauri/src/usage_script.rs` 中的按站点证书支持
- `src-tauri/Cargo.toml`、`Cargo.lock` 中对应的解码与证书解析依赖

升级前备份配置目录中的 `tls-pins.json`，在新安装上先查询额度。最终只保留一个正式安装目录、
一个运行实例，并让所有快捷方式指向该目录。不要让旧便携版和新安装版同时竞争本机 15721 端口。

原生回归命令（不调用模型；测试目录使用一次性路径）：

```powershell
cd src-tauri
$env:CC_SWITCH_TEST_HOME = Join-Path ([IO.Path]::GetTempPath()) ("cc-switch-tests-" + [guid]::NewGuid())
# 仅在当前测试进程中让 loopback 绕过系统代理；保留原来的变量以便恢复。
$previousNoProxy = $env:NO_PROXY
$env:NO_PROXY = "127.0.0.1,localhost,::1"
cargo +stable test --locked --lib proxy:: -- --test-threads=1
$env:NO_PROXY = $previousNoProxy
Remove-Item Env:CC_SWITCH_TEST_HOME
```

回归命令必须成功。跨版本时重新测试实际桌面 UI、四模型五档、图片、签名回放、
完整原生工具集、顶层组合 schema 的流式往返和压缩流程；同时以原生 Responses
供应商做 GPT 转发对照。不要将旧可执行文件覆盖到新版安装目录来“保留补丁”。
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
