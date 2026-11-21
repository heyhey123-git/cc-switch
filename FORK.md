# Codex + Claude 协议修复分支

本分支基于 CC Switch **v4.0.31**，基线提交为
`3c76d3bda670297dd8c27d7d9be9b18a4c9143f5`。

- [完整接入文档](docs/guides/codex-sub2api-production-zh.md)
- [无密钥配置示例](docs/examples/codex-sub2api/)
- [验证摘要](docs/validation/claude-protocol-v4031.json)

修复支持型号的 `xhigh` 被转换为 `max`、Haiku 5.5 被识别成旧式
thinking，以及部分新型号被发送非法 `thinking: disabled` 的问题。
保留旧型号的兼容行为及已有 thinking 签名。Adaptive 模型可以跳过思考块，
对应的合法工具结果仍能继续回传，不要求伪造或补充 thinking 签名。

首次验证结果：91 项原生测试、43 项本地协议检查通过，覆盖四个型号的五档推理、
图片、工具调用、thinking 签名回放、压缩及压缩后继续工作。协议检查使用本地模拟
上游，没有调用付费模型。它们不代表某个商家账户已通过实网鉴权或真实缓存验收。
后续修正 adaptive 工具续接后，原生回归增加至 92 项通过；
实网验收结果见[独立记录](docs/validation/claude-live-acceptance-2026-11-21.md)。

生产构建必须包含前端资源。使用 `pnpm build`，或者按接入文档中的显式
`tauri/custom-protocol` 命令构建；单独执行默认 `cargo build --release`
可能让窗口继续访问开发服务器。

本分支只提供源码和文档，不发布安装包。官方更新可能替换本地构建；
请按接入文档迁移补丁并重新验证，勿用旧版可执行文件覆盖新版。
