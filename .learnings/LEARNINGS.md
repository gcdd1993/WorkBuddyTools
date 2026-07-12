# Project Learnings

## [LRN-20260712-001] correction

**Logged**: 2026-07-12T00:00:00+08:00
**Priority**: high
**Status**: resolved
**Area**: config

### Summary
区分 WorkBuddy 数据目录与 WorkBuddyTools 本程序配置目录，避免配置和数据来源错位。

### Details
最初误把“本程序配置存储目录”实现为可配置的 WorkBuddy 数据目录。正确边界是：WorkBuddy 的模型、供应商、数据库与会话数据固定来自 `%USERPROFILE%\.workbuddy`；WorkBuddyTools 自身设置固定保存到其子目录 `workbuddy-tools/settings.json`，且不提供配置目录选择功能。这样只维护一份本程序配置，并确保模型与会话始终使用 WorkBuddy 的默认数据源。

### Suggested Action
移除 WorkBuddy 数据目录及 WorkBuddyTools 配置目录的可配置入口；在后端固定 WorkBuddy 数据根目录，并将本程序设置路径固定为 `%USERPROFILE%\.workbuddy\workbuddy-tools\settings.json`，同步调整前端设置界面与文案。

### Resolution
2026-07-12：代码已按固定路径完成修正，未提交。

### Metadata
- Source: user_feedback
- Related Files: src-tauri/src/settings.rs, src-tauri/src/lib.rs, src/main.tsx
- Tags: configuration-boundary, workbuddy-home, settings-path

---
