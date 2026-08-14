use chrono::Utc;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

use crate::{
    codebuddy_dir,
    sessions::{
        BatchReplaceSessionCwdInput, BatchReplaceSessionCwdPreviewResult,
        BatchReplaceSessionCwdResult, SessionCwdReplacementPreview, UpdateSessionInput,
    },
};

/// CodeBuddy 会话摘要，对应前端 WorkBuddySessionSummary 的字段子集。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeBuddySessionSummary {
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub status: String,
    pub model: String,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub last_activity_at: Option<i64>,
    pub size_bytes: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteCodeBuddySessionResult {
    pub session_id: String,
    pub deleted_at: i64,
    pub trash_dir: String,
    pub moved_items: usize,
    pub warning: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ConversationIndex {
    #[serde(default)]
    conversations: Vec<ConversationEntry>,
    #[serde(default)]
    current: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ConversationEntry {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    last_message_at: String,
}

enum CwdReplacer {
    Literal(String),
    Regex(Regex),
}

/// CodeBuddy 会话根目录：
/// %LOCALAPPDATA%\CodeBuddyExtension\Data\{userId}\VSCode\{userId}\history
fn codebuddy_history_root() -> Result<PathBuf, String> {
    let local_app_data = std::env::var_os("LOCALAPPDATA")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "无法读取 LOCALAPPDATA 环境变量".to_string())?;

    let data_dir = PathBuf::from(local_app_data)
        .join("CodeBuddyExtension")
        .join("Data");

    if !data_dir.exists() {
        return Err(format!("CodeBuddy 数据目录不存在：{}", data_dir.display()));
    }

    // 查找第一个包含 VSCode\{userId}\history 的用户目录
    for entry in
        fs::read_dir(&data_dir).map_err(|err| format!("读取 CodeBuddy 数据目录失败：{err}"))?
    {
        let entry = entry.map_err(|err| format!("读取目录条目失败：{err}"))?;
        let user_dir = entry.path();
        let vscode_dir = user_dir.join("VSCode");
        if !vscode_dir.exists() {
            continue;
        }
        // 在 VSCode 下查找与用户目录同名的子目录
        for vscode_entry in
            fs::read_dir(&vscode_dir).map_err(|err| format!("读取 VSCode 目录失败：{err}"))?
        {
            let vscode_entry =
                vscode_entry.map_err(|err| format!("读取 VSCode 目录条目失败：{err}"))?;
            let inner_dir = vscode_entry.path();
            let history_dir = inner_dir.join("history");
            if history_dir.exists() && history_dir.is_dir() {
                return Ok(history_dir);
            }
        }
    }

    Err(format!(
        "未找到 CodeBuddy 会话历史目录，请确认 CodeBuddy 已正确安装并使用过"
    ))
}

#[tauri::command]
pub fn list_codebuddy_sessions() -> Result<Vec<CodeBuddySessionSummary>, String> {
    let history_root = codebuddy_history_root()?;
    let mut sessions = Vec::new();

    // 遍历 history 下的每个 workspace hash 目录
    for entry in
        fs::read_dir(&history_root).map_err(|err| format!("读取会话历史目录失败：{err}"))?
    {
        let entry = entry.map_err(|err| format!("读取目录条目失败：{err}"))?;
        let workspace_dir = entry.path();
        if !workspace_dir.is_dir() {
            continue;
        }

        let index_path = workspace_dir.join("index.json");
        if !index_path.exists() {
            continue;
        }

        let content = match fs::read_to_string(&index_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let index: ConversationIndex = match serde_json::from_str(&content) {
            Ok(idx) => idx,
            Err(_) => continue,
        };

        for conv in &index.conversations {
            let conv_dir = workspace_dir.join(&conv.id);
            let size_bytes = if conv_dir.exists() {
                path_size(&conv_dir)
            } else {
                0
            };

            let created_at = parse_iso_to_millis(&conv.created_at);
            let last_activity_at = parse_iso_to_millis(&conv.last_message_at);

            sessions.push(CodeBuddySessionSummary {
                id: conv.id.clone(),
                title: if conv.name.is_empty() {
                    "未命名会话".to_string()
                } else {
                    conv.name.clone()
                },
                cwd: resolve_session_workspace(&history_root, &workspace_dir, &conv.id)
                    .unwrap_or_else(|| workspace_dir.to_string_lossy().to_string()),
                status: "completed".to_string(),
                model: String::new(),
                created_at,
                updated_at: last_activity_at,
                last_activity_at,
                size_bytes,
            });
        }
    }

    // 按最后活动时间降序排列
    sessions.sort_by(|a, b| b.last_activity_at.cmp(&a.last_activity_at));
    Ok(sessions)
}

#[tauri::command]
pub fn update_codebuddy_session(input: UpdateSessionInput) -> Result<(), String> {
    validate_session_id(&input.session_id)?;
    let title = required_trimmed(input.title, "会话名称")?;
    let cwd = required_trimmed(input.cwd, "工作目录")?;
    let history_root = codebuddy_history_root()?;
    let (workspace_dir, index_path, mut index, conversation_index) =
        locate_session(&history_root, &input.session_id)?;
    let old_cwd = resolve_session_workspace(&history_root, &workspace_dir, &input.session_id)
        .unwrap_or_else(|| workspace_dir.to_string_lossy().to_string());

    if old_cwd != cwd {
        update_session_workspace_metadata(
            &history_root,
            &workspace_dir,
            &input.session_id,
            &old_cwd,
            &cwd,
        )?;
    }
    index["conversations"][conversation_index]["name"] = Value::String(title);
    write_json_atomic(&index_path, &index)
}

#[tauri::command]
pub fn preview_codebuddy_session_cwd_replace(
    input: BatchReplaceSessionCwdInput,
) -> Result<BatchReplaceSessionCwdPreviewResult, String> {
    validate_batch_replace_input(&input)?;
    let replacer = build_cwd_replacer(&input)?;
    let sessions = list_codebuddy_sessions()?;
    let mut matches = Vec::new();
    let mut unchanged = 0;

    for session_id in &input.session_ids {
        let Some(session) = sessions.iter().find(|session| session.id == *session_id) else {
            unchanged += 1;
            continue;
        };
        let new_cwd = replace_cwd(&session.cwd, &input.replacement, &replacer);
        if new_cwd == session.cwd {
            unchanged += 1;
            continue;
        }
        if new_cwd.trim().is_empty() {
            return Err(format!("会话 {} 替换后的工作目录不能为空", session.id));
        }
        matches.push(SessionCwdReplacementPreview {
            session_id: session.id.clone(),
            title: session.title.clone(),
            old_cwd: session.cwd.clone(),
            new_cwd,
        });
    }

    Ok(BatchReplaceSessionCwdPreviewResult {
        matches,
        skipped_working: 0,
        unchanged,
    })
}

#[tauri::command]
pub fn batch_replace_codebuddy_session_cwd(
    input: BatchReplaceSessionCwdInput,
) -> Result<BatchReplaceSessionCwdResult, String> {
    let preview = preview_codebuddy_session_cwd_replace(BatchReplaceSessionCwdInput {
        session_ids: input.session_ids.clone(),
        search: input.search.clone(),
        replacement: input.replacement.clone(),
        is_regex: input.is_regex,
        expected_matches: Vec::new(),
    })?;
    validate_replacement_expectations(&preview.matches, &input.expected_matches)?;
    let history_root = codebuddy_history_root()?;

    for replacement in &preview.matches {
        let (workspace_dir, _, _, _) = locate_session(&history_root, &replacement.session_id)?;
        update_session_workspace_metadata(
            &history_root,
            &workspace_dir,
            &replacement.session_id,
            &replacement.old_cwd,
            &replacement.new_cwd,
        )?;
    }

    Ok(BatchReplaceSessionCwdResult {
        updated: preview.matches.len(),
        skipped_working: 0,
        unchanged: preview.unchanged,
    })
}

#[tauri::command]
pub fn delete_codebuddy_session(
    session_id: String,
) -> Result<DeleteCodeBuddySessionResult, String> {
    if session_id.is_empty() || session_id.len() > 200 {
        return Err("无效的会话 ID".to_string());
    }

    let history_root = codebuddy_history_root()?;
    let mut found_session_dir: Option<PathBuf> = None;
    let mut found_workspace_dir: Option<PathBuf> = None;

    // 在所有 workspace 目录中查找会话
    for entry in
        fs::read_dir(&history_root).map_err(|err| format!("读取会话历史目录失败：{err}"))?
    {
        let entry = entry.map_err(|err| format!("读取目录条目失败：{err}"))?;
        let workspace_dir = entry.path();
        if !workspace_dir.is_dir() {
            continue;
        }
        let session_dir = workspace_dir.join(&session_id);
        if session_dir.exists() {
            found_session_dir = Some(session_dir);
            found_workspace_dir = Some(workspace_dir);
            break;
        }
    }

    let session_dir = found_session_dir.ok_or_else(|| format!("未找到会话 {session_id}"))?;
    let workspace_dir =
        found_workspace_dir.ok_or_else(|| "无法确定会话所属的工作区目录".to_string())?;

    let deleted_at = Utc::now().timestamp_millis();
    let trash_dir = codebuddy_dir()?
        .join("session-trash")
        .join(deleted_at.to_string())
        .join(&session_id);

    fs::create_dir_all(&trash_dir).map_err(|err| format!("创建回收站目录失败：{err}"))?;

    // 移动会话目录到回收站
    let dest = trash_dir.join("files").join(&session_id);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("创建回收目录失败：{err}"))?;
    }
    fs::rename(&session_dir, &dest).map_err(|err| format!("移动会话文件到回收站失败：{err}"))?;

    let moved_items = 1;

    // 从 workspace 的 index.json 中移除该会话
    let index_path = workspace_dir.join("index.json");
    let mut warning: Option<String> = None;
    if index_path.exists() {
        if let Err(err) = remove_conversation_from_index(&index_path, &session_id) {
            warning = Some(format!("会话已移入回收站，但更新索引文件失败：{err}"));
        }
    }

    Ok(DeleteCodeBuddySessionResult {
        session_id,
        deleted_at,
        trash_dir: trash_dir.to_string_lossy().to_string(),
        moved_items,
        warning,
    })
}

fn remove_conversation_from_index(index_path: &Path, session_id: &str) -> Result<(), String> {
    let content =
        fs::read_to_string(index_path).map_err(|err| format!("读取 index.json 失败：{err}"))?;
    let mut value: Value =
        serde_json::from_str(&content).map_err(|err| format!("解析 index.json 失败：{err}"))?;

    if let Some(conversations) = value.get_mut("conversations").and_then(Value::as_array_mut) {
        conversations.retain(|conv| conv.get("id").and_then(Value::as_str) != Some(session_id));
    }

    // 如果 current 指向被删除的会话，清空它
    if value.get("current").and_then(Value::as_str) == Some(session_id) {
        if let Some(obj) = value.as_object_mut() {
            obj.insert("current".to_string(), Value::String(String::new()));
        }
    }

    let parent = index_path
        .parent()
        .ok_or_else(|| "无效的 index.json 路径".to_string())?;
    let temp_path = parent.join(format!("index.json.{}.tmp", std::process::id()));
    let serialized = serde_json::to_string_pretty(&value)
        .map_err(|err| format!("序列化 index.json 失败：{err}"))?;
    fs::write(&temp_path, format!("{serialized}\n"))
        .map_err(|err| format!("写入临时索引文件失败：{err}"))?;
    fs::rename(&temp_path, index_path).map_err(|err| format!("替换 index.json 失败：{err}"))?;
    Ok(())
}

fn locate_session(
    history_root: &Path,
    session_id: &str,
) -> Result<(PathBuf, PathBuf, Value, usize), String> {
    for entry in fs::read_dir(history_root).map_err(|err| format!("读取会话历史目录失败：{err}"))?
    {
        let workspace_dir = entry
            .map_err(|err| format!("读取目录条目失败：{err}"))?
            .path();
        let index_path = workspace_dir.join("index.json");
        if !workspace_dir.is_dir() || !index_path.exists() {
            continue;
        }
        let content = fs::read_to_string(&index_path)
            .map_err(|err| format!("读取 index.json 失败：{err}"))?;
        let index: Value =
            serde_json::from_str(&content).map_err(|err| format!("解析 index.json 失败：{err}"))?;
        let conversation_index = index
            .get("conversations")
            .and_then(Value::as_array)
            .and_then(|conversations| {
                conversations.iter().position(|conversation| {
                    conversation.get("id").and_then(Value::as_str) == Some(session_id)
                })
            });
        if let Some(conversation_index) = conversation_index {
            return Ok((workspace_dir, index_path, index, conversation_index));
        }
    }
    Err(format!("未找到会话 {session_id}"))
}

fn resolve_session_workspace(
    history_root: &Path,
    workspace_dir: &Path,
    session_id: &str,
) -> Option<String> {
    let workspace_key = workspace_dir.file_name()?;
    let checkpoint_dir = history_root
        .parent()?
        .join("check-point")
        .join(workspace_key)
        .join(session_id);
    let mut metadata_paths = find_metadata_paths(&checkpoint_dir);
    metadata_paths.sort_by_key(|path| {
        fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok()
    });
    metadata_paths.reverse();

    for path in metadata_paths {
        let value: Value = fs::read_to_string(path)
            .ok()
            .and_then(|content| serde_json::from_str(&content).ok())?;
        if let Some(workspace) = find_string_field(&value, "workspace") {
            return Some(workspace.to_string());
        }
    }
    None
}

fn update_session_workspace_metadata(
    history_root: &Path,
    workspace_dir: &Path,
    session_id: &str,
    old_cwd: &str,
    new_cwd: &str,
) -> Result<(), String> {
    let workspace_key = workspace_dir
        .file_name()
        .ok_or_else(|| "无法确定 CodeBuddy 工作区标识".to_string())?;
    let checkpoint_dir = history_root
        .parent()
        .ok_or_else(|| "无法确定 CodeBuddy 数据目录".to_string())?
        .join("check-point")
        .join(workspace_key)
        .join(session_id);
    let mut changed_files = Vec::new();

    for path in find_metadata_paths(&checkpoint_dir) {
        let content = fs::read_to_string(&path)
            .map_err(|err| format!("读取 checkpoint 元数据失败：{err}"))?;
        let mut value: Value = serde_json::from_str(&content)
            .map_err(|err| format!("解析 checkpoint 元数据失败：{err}"))?;
        if replace_string_field(&mut value, "workspace", old_cwd, new_cwd) > 0 {
            changed_files.push((path, value));
        }
    }
    if changed_files.is_empty() {
        return Err(format!("会话 {session_id} 没有可更新的工作目录元数据"));
    }
    for (path, value) in changed_files {
        write_json_atomic(&path, &value)?;
    }
    Ok(())
}

fn find_metadata_paths(root: &Path) -> Vec<PathBuf> {
    if !root.exists() {
        return Vec::new();
    }
    WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file() && entry.file_name() == "meta.json")
        .map(|entry| entry.into_path())
        .collect()
}

fn find_string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    match value {
        Value::Object(object) => object.get(key).and_then(Value::as_str).or_else(|| {
            object
                .values()
                .find_map(|child| find_string_field(child, key))
        }),
        Value::Array(items) => items.iter().find_map(|item| find_string_field(item, key)),
        _ => None,
    }
}

fn replace_string_field(value: &mut Value, key: &str, old: &str, new: &str) -> usize {
    match value {
        Value::Object(object) => {
            let mut changed = 0;
            if object.get(key).and_then(Value::as_str) == Some(old) {
                object.insert(key.to_string(), Value::String(new.to_string()));
                changed += 1;
            }
            changed
                + object
                    .values_mut()
                    .map(|child| replace_string_field(child, key, old, new))
                    .sum::<usize>()
        }
        Value::Array(items) => items
            .iter_mut()
            .map(|item| replace_string_field(item, key, old, new))
            .sum(),
        _ => 0,
    }
}

fn validate_session_id(session_id: &str) -> Result<(), String> {
    if session_id.is_empty()
        || session_id.len() > 200
        || !session_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    {
        return Err("无效的会话 ID".to_string());
    }
    Ok(())
}

fn required_trimmed(value: String, label: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{label}不能为空"));
    }
    Ok(trimmed.to_string())
}

fn validate_batch_replace_input(input: &BatchReplaceSessionCwdInput) -> Result<(), String> {
    if input.session_ids.is_empty() {
        return Err("没有可处理的会话".to_string());
    }
    if input.session_ids.len() > 10_000 {
        return Err("单次最多处理 10000 个会话".to_string());
    }
    for session_id in &input.session_ids {
        validate_session_id(session_id)?;
    }
    if input.search.is_empty() {
        return Err("查找内容不能为空".to_string());
    }
    if input.search.len() > 2000 || input.replacement.len() > 4000 {
        return Err("查找或替换内容过长".to_string());
    }
    Ok(())
}

fn build_cwd_replacer(input: &BatchReplaceSessionCwdInput) -> Result<CwdReplacer, String> {
    if input.is_regex {
        return Regex::new(&input.search)
            .map(CwdReplacer::Regex)
            .map_err(|error| format!("正则表达式无效：{error}"));
    }
    Ok(CwdReplacer::Literal(input.search.clone()))
}

fn replace_cwd(value: &str, replacement: &str, replacer: &CwdReplacer) -> String {
    match replacer {
        CwdReplacer::Literal(search) => value.replace(search, replacement),
        CwdReplacer::Regex(regex) => regex.replace_all(value, replacement).into_owned(),
    }
}

fn validate_replacement_expectations(
    replacements: &[SessionCwdReplacementPreview],
    expectations: &[crate::sessions::SessionCwdReplacementExpectation],
) -> Result<(), String> {
    if replacements.len() != expectations.len()
        || !replacements.iter().all(|replacement| {
            expectations.iter().any(|expectation| {
                expectation.session_id == replacement.session_id
                    && expectation.old_cwd == replacement.old_cwd
                    && expectation.new_cwd == replacement.new_cwd
            })
        })
    {
        return Err("会话工作目录在预览后发生变化，请重新预览".to_string());
    }
    Ok(())
}

fn write_json_atomic(path: &Path, value: &Value) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "无效的 JSON 文件路径".to_string())?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|err| format!("创建临时 JSON 文件失败：{err}"))?;
    serde_json::to_writer_pretty(temporary.as_file_mut(), value)
        .map_err(|err| format!("写入临时 JSON 文件失败：{err}"))?;
    temporary
        .persist(path)
        .map_err(|err| format!("安全替换 JSON 文件失败：{}", err.error))?;
    Ok(())
}

fn parse_iso_to_millis(iso: &str) -> Option<i64> {
    if iso.is_empty() {
        return None;
    }
    chrono::DateTime::parse_from_rfc3339(iso)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

fn path_size(path: &Path) -> u64 {
    if path.is_file() {
        return fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    }
    WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| entry.metadata().ok().map(|m| m.len()))
        .sum()
}
