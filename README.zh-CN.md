# WorkBuddy Tools

[English](README.md) | [简体中文](README.zh-CN.md)

![Version](https://img.shields.io/badge/version-0.2.3-blue)
![Tauri](https://img.shields.io/badge/Tauri-2-24C8DB)
![Rust](https://img.shields.io/badge/Rust-2021-000000)
![React](https://img.shields.io/badge/React-18-61DAFB)

WorkBuddy Tools 是一个面向 Windows 的 WorkBuddy 桌面辅助工具，用于管理模型供应商、本机会话和 WebDAV 备份。它把原本需要手工修改 JSON 文件或 WorkBuddy SQLite 数据库的操作集中到一个更安全的界面中。

> 本项目是独立的辅助工具，不会替代 WorkBuddy。

## 功能截图

### 模型与供应商管理

![模型管理](docs/screenshots/model-management.png)
![供应商管理](docs/screenshots/model-provider.png)

### 会话管理

![会话管理](docs/screenshots/session-management.png)

### WebDAV 同步

![WebDAV 同步](docs/screenshots/webdav-sync.png)

## 主要功能

### 模型与供应商

- 读取 `%USERPROFILE%\.workbuddy\models.json` 中的 WorkBuddy 模型。
- 添加、更新和删除第三方 OpenAI 兼容模型。
- 在 `model-providers.json` 中独立保存供应商名称、API 地址和 API Key。
- 从供应商的 `/v1/models` 接口拉取可用模型。
- 自动推断工具调用、图片输入、推理和自定义协议能力。
- 优先使用供应商元数据，并通过内置模型数据库补齐输入、输出 token 上限。
- 修改前自动备份 `models.json`。

### 会话管理

- 直接从 `%USERPROFILE%\.workbuddy\workbuddy.db` 的 `sessions` 表读取有效会话。
- 按会话名称或工作目录搜索。
- 展示 `sessions.model` 中记录的模型。
- 编辑会话名称（`custom_title`）和工作目录（`cwd`）。
- 将未运行的会话移入 WorkBuddy 回收站。
- 正在运行的会话禁止编辑和删除。

### WebDAV 同步

- 将会话、引用的 Blob、产物索引、用户记忆 Markdown、便携个性化字段、模型和供应商配置打包同步。
- 支持智能合并、远端覆盖本机、本机覆盖远端三种策略。
- 可使用独立同步密码加密 ZIP 包。
- 远端覆盖本机前自动创建本地备份。
- 智能合并时不合并模型和供应商配置，保持本机配置不变。
- 多台设备默认工作空间根目录不同时，自动修复会话路径。

会话同步时会比较远端与本机的 `defaultWorkspacePath`。本机配置读取自：

```text
%USERPROFILE%\.workbuddy\app\app-config.json
```

如果路径不同，导入后会把 `sessions.cwd` 和 `workspaces.path` 中的远端根路径替换为本机根路径。例如：

```text
D:\OneDrive\WorkBuddy\WorkSpace\project-a
->
E:\OneDrive\WorkBuddy\WorkSpace\project-a
```

不会上传完整的 `app-config.json`；同步包只包含 `disableAgentTeams`、`personalization`，以及路径修复所需的默认工作空间路径元数据。

## 同步内容

| 数据 | 是否同步 | 说明 |
| --- | --- | --- |
| 会话元数据 | 是 | 从 `workbuddy.db` 导出，不上传数据库文件本身 |
| 项目会话文件 | 是 | `%USERPROFILE%\.workbuddy\projects\**\*.jsonl` |
| 会话引用的 Blob | 是 | 只打包同步会话实际引用的内容寻址文件 |
| 会话产物索引 | 是 | 按会话和稳定的产物标识合并 |
| 用户记忆与身份 | 是 | 根目录身份 Markdown 与 `memory/**/*.md`，排除备份文件 |
| 便携个性化配置 | 部分字段 | 仅 `disableAgentTeams` 与 `personalization` |
| 模型配置 | 是 | `models.json` |
| 供应商配置 | 是 | `model-providers.json`，可能包含 API Key |
| 默认工作空间路径 | 仅元数据 | 用于路径修复，不上传完整的 `app-config.json` |
| 运行时文件 | 否 | 排除 PID sidecar、缓存、日志、SQLite WAL 和 SHM 文件 |

启用加密时，远端文件名为 `workbuddy-sync.zip.enc`；未设置同步密码时为 `workbuddy-sync.zip`。明文同步包会让 WebDAV 服务端直接接触会话、用户记忆、附件、个性化配置、模型配置和 API Key，强烈建议启用加密。

## 配置文件

WorkBuddy 数据固定从 `%USERPROFILE%\.workbuddy` 读取：

```text
%USERPROFILE%\.workbuddy\
├── app\app-config.json
├── projects\
├── model-providers.json
├── models.json
└── workbuddy.db
```

WorkBuddy Tools 自身设置保存在：

```text
%USERPROFILE%\.workbuddy\workbuddy-tools\settings.json
```

WebDAV 凭据和可选同步密码目前以明文保存在该本地设置文件中，请确保 Windows 账户和文件权限安全。

## 环境要求

- Windows
- WorkBuddy 数据位于 `%USERPROFILE%\.workbuddy`
- 开发环境需要 Node.js、npm、Rust、Cargo 和 WebView2 Runtime
- 管理自定义模型时，供应商需要兼容 OpenAI 接口

供应商至少应支持：

```text
GET /v1/models
POST /v1/chat/completions
```

## 开发

安装依赖并启动 Tauri 应用：

```powershell
npm install
npm run tauri dev
```

常用检查命令：

```powershell
npm run typecheck
npm run test:layout
npm run test:provider-workflow
npm run test:runtime
npm run test:theme
cargo test --manifest-path src-tauri/Cargo.toml --lib
npm run build
```

构建不含安装包的 release 可执行文件：

```powershell
npm run tauri build -- --no-bundle
```

## 应用自动更新

应用启动后会自动检查 GitHub Releases，发现新版本时在顶部显示更新提示，并可在“应用设置 → 关于与更新”中下载安装。发布 `v*` 标签前，需要在 GitHub 仓库的 Actions Secrets 中配置：

- `TAURI_SIGNING_PRIVATE_KEY`：与 `src-tauri/tauri.conf.json` 中公钥配对的 Tauri updater 私钥内容。
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`：私钥密码；无密码密钥可留空。

发布前必须同步更新 `package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json` 和 `VERSION` 中的版本号。推送版本标签后，Release 工作流会生成签名更新包和 `latest.json`。请妥善备份私钥；丢失私钥后，已经安装的旧版本无法验证使用新密钥签名的更新。

## 技术栈

- Tauri 2
- Rust 与 SQLite
- React 18 与 TypeScript
- Vite

## 隐私说明

- 供应商 API Key 保存在本机 `model-providers.json`。
- 同步的供应商配置可能包含 API Key。
- 会话同步包包含对话数据。
- 建议使用强度足够且与 WebDAV 密码不同的同步密码，并确保所有需要恢复数据的设备都能取得该密码。
- 模型能力和 token 上限可能来自自动推断；对精确限制有要求时，请以供应商最新文档为准。
