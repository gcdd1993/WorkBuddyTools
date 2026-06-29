# WorkBuddy 模型配置工具

一个基于 Tauri 2 + Rust + React 的桌面工具，用来维护 WorkBuddy 的第三方 OpenAI 兼容模型配置。

工具会读取 WorkBuddy 的模型配置文件，管理自定义供应商，从供应商接口拉取可用模型，并把选中的模型写入 WorkBuddy。

## 功能

- 显示 WorkBuddy 已配置模型列表。
- 管理第三方模型供应商。
- 支持 OpenAI 兼容接口。
- 通过供应商 `/v1/models` 拉取模型列表。
- 自动推断模型能力：
  - `supportsToolCall`
  - `supportsImages`
  - `supportsReasoning`
  - `useCustomProtocol`
- 自动填充 token 上限：
  - `maxInputTokens`
  - `maxOutputTokens`
- 将选中的模型写入 WorkBuddy 配置。
- 写入 `models.json` 前自动创建备份。
- 使用带齿轮角标的 WorkBuddy 图标。

## 配置文件

WorkBuddy 模型配置：

```text
C:\Users\PC\.workbuddy\models.json
```

本工具维护的供应商配置：

```text
C:\Users\PC\.workbuddy\model-providers.json
```

供应商配置与 WorkBuddy 的 `models.json` 放在同一目录，但不会混写到 WorkBuddy 原配置里。

## 写入规则

添加模型到 WorkBuddy 时，生成的模型配置格式如下：

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

规则：

- `id` 使用供应商返回的原始模型 ID。
- `name` 使用 `${供应商名称}-${模型ID}`。
- `vendor` 固定为 `Custom`。
- `url` 自动规范化为 `/v1/chat/completions`。
- 如果模型 ID 已存在，则更新该条模型配置。
- 如果模型 ID 不存在，则追加到 `models.json`。
- 写入前会备份原文件，例如：

```text
models.json.20260629T120000Z.bak
```

## Token 上限来源

很多 OpenAI 兼容供应商的 `/v1/models` 只返回模型 ID，不返回上下文长度或最大输出。

本工具使用两级策略：

1. 优先读取供应商 `/v1/models` 返回的字段，例如：
   - `maxInputTokens`
   - `max_input_tokens`
   - `contextLength`
   - `context_length`
   - `maxOutputTokens`
   - `max_output_tokens`
   - `maxCompletionTokens`
   - `max_completion_tokens`
2. 如果供应商没有返回，则使用内置模型数据库按模型 ID 匹配补齐。

内置数据库来自 Kivio 同类实现思路，匹配顺序为：

- 精确匹配。
- 去掉 `provider/model` 前缀后匹配。
- 最长前缀匹配。
- 最长包含匹配。

例如 `deepseek-ai/DeepSeek-V4-Pro` 可以补齐：

```text
maxInputTokens = 1048576
maxOutputTokens = 384000
```

## 使用方式

1. 打开工具。
2. 切换到“供应商”Tab。
3. 填写供应商名称、API 请求地址、API Key。
4. 点击“添加供应商”。
5. 选择供应商并点击“拉取模型”。
6. 勾选要添加到 WorkBuddy 的模型。
7. 点击“添加到 WorkBuddy”。
8. 切换到“模型列表”查看写入结果。

WorkBuddy 不需要重启即可读取新的模型配置。

## 开发运行

前置环境：

- Node.js
- npm
- Rust / Cargo
- WebView2 Runtime

安装依赖：

```powershell
npm install
```

开发模式：

```powershell
npm run tauri dev
```

前端构建：

```powershell
npm run build
```

Rust 检查：

```powershell
cargo check --manifest-path src-tauri/Cargo.toml
```

Rust 测试：

```powershell
cargo test --manifest-path src-tauri/Cargo.toml --lib
```

## 构建可执行文件

构建 release exe：

```powershell
npm run tauri build -- --no-bundle
```

输出文件：

```text
D:\WorkSpace\personal\WorkBuddyTools\src-tauri\target\release\workbuddy-model-config.exe
```

说明：

- `--no-bundle` 会生成可执行文件，不生成安装包。
- MSI 安装包依赖本机 WiX 工具链，当前项目主要验证 release exe。

## 项目结构

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

## 验证命令

提交或发布前建议执行：

```powershell
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
npm run typecheck
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml --lib
npm run build
npm run tauri build -- --no-bundle
```

## 注意事项

- API Key 会保存在 `model-providers.json` 中，请注意本机文件权限。
- `models.json` 写入前会自动备份，但仍建议在大批量添加模型前确认当前配置可用。
- 模型能力和 token 上限可能来自内置数据库匹配，供应商真实限制变化时需要更新 `modelDatabase.json`。
- 仅支持 OpenAI 兼容接口；非 OpenAI 兼容供应商暂不处理。
