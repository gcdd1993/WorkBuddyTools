# WorkBuddy 模型配置工具

![version](https://img.shields.io/badge/version-0.2.2-blue)
![Tauri](https://img.shields.io/badge/Tauri-2-24C8DB)
![Rust](https://img.shields.io/badge/Rust-2021-000000)
![React](https://img.shields.io/badge/React-18-61DAFB)

WorkBuddy 模型配置工具是一个 Tauri 2 + Rust + React 桌面应用，用来维护 WorkBuddy 的第三方 OpenAI 兼容模型配置。

它面向已经在使用 WorkBuddy、并且需要接入自定义模型供应商的用户。你可以在一个独立工具里保存供应商、拉取模型、检查模型能力，并把选中的模型写入 WorkBuddy 的 `models.json`。

## Design stance

这个工具只做一件事：把第三方 OpenAI 兼容模型，稳定地写进 WorkBuddy 能识别的配置里。

它不会替代 WorkBuddy，也不会尝试成为通用模型管理平台。它的重点是减少手工编辑 JSON 的风险：路径固定、写入规则明确、更新前备份、模型能力和 token 上限尽量自动补齐。

## Core idea

WorkBuddy 的模型配置在本机文件中：

```text
C:\Users\PC\.workbuddy\models.json
```

WorkBuddy 数据目录固定为 `%USERPROFILE%\.workbuddy`，常见示例为 `C:\Users\PC\.workbuddy`。本程序自身设置固定保存到 `%USERPROFILE%\.workbuddy\workbuddy-tools\settings.json`。

本工具额外维护供应商配置：

```text
C:\Users\PC\.workbuddy\model-providers.json
```

两类配置放在同一个 `.workbuddy` 目录下，但用途不同：

- `models.json` 是 WorkBuddy 读取的模型列表。
- `model-providers.json` 是本工具保存的供应商列表和 API Key。

添加模型时，工具会从供应商的 `/v1/models` 拉取模型 ID，然后写入 WorkBuddy 所需的 `Custom` 模型配置。

## Where it fits

适合这些场景：

- 你想给 WorkBuddy 添加第三方 OpenAI 兼容模型。
- 你不想手工维护 `models.json`。
- 你有多个供应商，需要保存并切换 API 地址和 API Key。
- 你希望从供应商接口拉取模型列表，而不是手动复制模型 ID。
- 你希望写入前自动备份 WorkBuddy 配置。

## Where it does not fit

不适合这些场景：

- 供应商不是 OpenAI 兼容接口。
- 你需要管理非 WorkBuddy 的模型配置。
- 你需要团队级权限管理或集中式远程配置下发。
- 你需要安装包下载页或自动更新机制。当前 README 不提供这些信息。

## Core features

### WorkBuddy 模型管理

- 读取并展示 WorkBuddy 已配置模型。
- 添加新的第三方模型到 `models.json`。
- 模型 ID 已存在时更新原配置。
- 从 WorkBuddy 模型列表删除不需要的模型。
- 写入、更新或删除前备份 `models.json`。

备份文件示例：

```text
models.json.20260629T120000Z.bak
```

### 供应商管理

- 维护第三方模型供应商配置。
- 保存供应商名称、API 请求地址和 API Key。
- 从供应商 `/v1/models` 拉取可用模型。
- 将供应商配置写入 `model-providers.json`，不混写到 WorkBuddy 原始模型配置中。

### WebDAV ZIP 同步

工具可以把 WorkBuddy 会话和当前模型配置打包为 ZIP，并通过 WebDAV 在多台设备之间同步。同步包包含：

- `%USERPROFILE%\.workbuddy\projects\**\*.jsonl` 中的项目会话。
- `%USERPROFILE%\.workbuddy\models.json` 中的 WorkBuddy 模型配置。
- `%USERPROFILE%\.workbuddy\model-providers.json` 中的供应商配置。

同步不会包含 WorkBuddy 的原始数据库及其 `WAL`、`SHM` 文件，也不会包含 `sessions` 目录中的 PID sidecar、缓存或日志。上传前，ZIP 会使用独立的同步口令加密，远端文件名为：

```text
workbuddy-sync.zip.enc
```

远端公开 manifest 只记录同步代次、设备、文件路径、大小和校验值等传输元数据，不包含 API Key、WebDAV 密码或 ZIP 加密口令。

支持三种同步策略：

- **智能合并**：合并本机与远端会话和配置；发生冲突时保留冲突副本，并优先保留本机 API Key 等敏感字段。
- **远端覆盖本机**：使用远端同步包替换本机内容。覆盖前会备份到 `%USERPROFILE%\.workbuddy\sync-backups\<timestamp>\`。
- **本机覆盖远端**：将当前本机内容打包并作为新的远端版本上传。

WebDAV 用户名和密码仅用于连接 WebDAV 服务；同步口令用于加密和解密 ZIP 内容。两者用途不同，建议使用不同密码并妥善保存同步口令，口令丢失后无法解密远端同步包。

### 自动推断模型能力

工具会根据供应商返回信息和模型 ID，尽量补齐这些字段：

- `supportsToolCall`
- `supportsImages`
- `supportsReasoning`
- `useCustomProtocol`

这些字段用于让 WorkBuddy 了解模型是否支持工具调用、图片输入、推理能力或自定义协议。

### 自动补齐 token 上限

很多 OpenAI 兼容供应商的 `/v1/models` 只返回模型 ID，不返回上下文长度或最大输出 token。

工具使用两级策略：

1. 优先读取供应商返回字段，例如 `maxInputTokens`、`contextLength`、`maxOutputTokens`、`maxCompletionTokens` 等。
2. 如果供应商没有返回，则使用内置模型数据库按模型 ID 匹配补齐。

内置匹配顺序：

- 精确匹配。
- 去掉 `provider/model` 前缀后匹配。
- 最长前缀匹配。
- 最长包含匹配。

例如 `deepseek-ai/DeepSeek-V4-Pro` 可以补齐：

```text
maxInputTokens = 1048576
maxOutputTokens = 384000
```

## 写入格式

添加模型到 WorkBuddy 时，生成的配置类似：

```json
{
  "id": "deepseek-v4-flash",
  "name": "供应商名称-deepseek-v4-flash",
  "vendor": "Custom",
  "url": "https://example.com/v1/chat/completions",
  "apiKey": "sk-...",
  "supportsToolCall": true,
  "supportsImages": false,
  "supportsReasoning": true,
  "useCustomProtocol": false,
  "maxInputTokens": 1048576,
  "maxOutputTokens": 131072
}
```

写入规则：

- `id` 使用供应商返回的原始模型 ID。
- `name` 使用 `${供应商名称}-${模型ID}`。
- `vendor` 固定为 `Custom`。
- `url` 自动规范化为 `/v1/chat/completions`。
- 如果模型 ID 已存在，则更新该条配置。
- 如果模型 ID 不存在，则追加到 `models.json`。

## Installation

当前项目尚未提供 Release 下载地址或安装包说明。

如果你从源码运行，需要准备：

- Node.js
- npm
- Rust / Cargo
- WebView2 Runtime

安装依赖：

```powershell
npm install
```

## Quick start

1. 启动开发版应用：

   ```powershell
   npm run tauri dev
   ```

2. 打开 **供应商** 页面。
3. 填写供应商名称、API 请求地址和 API Key。
4. 点击 **添加供应商**。
5. 选择供应商并点击 **拉取模型**。
6. 勾选要添加到 WorkBuddy 的模型。
7. 点击 **添加到 WorkBuddy**。
8. 切换到 **模型列表** 查看写入结果。
9. 在 **模型列表** 中删除不再需要的模型。

WorkBuddy 通常可以直接读取更新后的模型配置；如果界面没有刷新，重新打开 WorkBuddy 再确认。

## Compatibility

当前工具面向 Windows 上的 WorkBuddy 配置目录，默认目录是 `%USERPROFILE%\.workbuddy`：

```text
C:\Users\PC\.workbuddy\
```

WorkBuddy 数据固定从该目录读取；本程序自身设置固定保存到 `%USERPROFILE%\.workbuddy\workbuddy-tools\settings.json`。

模型供应商需要提供 OpenAI 兼容接口，至少应支持：

```text
GET /v1/models
POST /v1/chat/completions
```

非 OpenAI 兼容供应商暂不处理。

## Tech stack

- Tauri 2：桌面应用外壳。
- Rust：本地文件读写、备份、供应商请求、模型写入逻辑。
- React 18：前端界面。
- TypeScript：前端类型检查。
- Vite：前端开发和构建。

项目结构：

```text
.
├── src/                         # React 前端
├── src-tauri/                   # Tauri / Rust 后端
│   ├── src/lib.rs               # 文件读写、供应商管理、模型拉取、写入 WorkBuddy
│   ├── resources/modelDatabase.json
│   └── icons/                   # 应用图标资源
├── package.json
├── vite.config.ts
└── README.md
```

## Development

常用命令：

```powershell
npm run tauri dev
npm run build
npm run typecheck
npm run tauri build -- --no-bundle
```

测试命令：

```powershell
npm run test:layout
npm run test:provider-workflow
npm run test:runtime
npm run test:theme
```

Rust 检查和测试：

```powershell
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml --lib
```

提交或发布前建议执行：

```powershell
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
npm run typecheck
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml --lib
npm run build
npm run tauri build -- --no-bundle
```

构建 release 可执行文件：

```powershell
npm run tauri build -- --no-bundle
```

`--no-bundle` 会生成可执行文件，不生成安装包。

## Privacy and notes

- API Key 会保存在本机 `model-providers.json` 中，请注意本机文件权限。
- WebDAV 同步包会加密上传，但解密后的内容包含模型配置和 API Key；请使用足够强的同步口令。
- 写入 `models.json` 前会自动备份，但批量添加模型前仍建议确认当前配置可用。
- 模型能力和 token 上限可能来自内置数据库匹配。供应商真实限制变化时，需要更新 `src-tauri/resources/modelDatabase.json`。
