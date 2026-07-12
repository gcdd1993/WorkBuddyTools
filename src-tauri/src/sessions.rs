use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

use crate::workbuddy_dir;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkBuddySessionSummary {
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
pub struct DeleteSessionResult {
    pub session_id: String,
    pub deleted_at: i64,
    pub trash_dir: String,
    pub moved_items: usize,
    pub recent_index_updated: bool,
    pub warning: Option<String>,
}

#[derive(Debug)]
struct SessionRecord {
    id: String,
    cwd: String,
    title: Option<String>,
    custom_title: Option<String>,
    status: Option<String>,
    created_at: Option<i64>,
    updated_at: Option<i64>,
    last_activity_at: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrashMetadata {
    session_id: String,
    deleted_at: i64,
    original_paths: Vec<String>,
}

#[tauri::command]
pub fn list_workbuddy_sessions() -> Result<Vec<WorkBuddySessionSummary>, String> {
    let root = workbuddy_dir()?;
    let connection = open_database(&root)?;
    let mut statement = connection
        .prepare(
            "SELECT id, cwd, title, custom_title, status, created_at, updated_at, last_activity_at
             FROM sessions WHERE deleted_at IS NULL ORDER BY COALESCE(last_activity_at, updated_at, created_at) DESC",
        )
        .map_err(|error| format!("读取会话表失败：{error}"))?;

    let records = statement
        .query_map([], map_session)
        .map_err(|error| format!("查询会话失败：{error}"))?;
    let mut sessions = Vec::new();
    for record in records {
        let record = record.map_err(|error| format!("解析会话记录失败：{error}"))?;
        let paths = find_session_paths(&root, &record.id)?;
        sessions.push(WorkBuddySessionSummary {
            id: record.id,
            title: non_empty(record.custom_title)
                .or_else(|| non_empty(record.title))
                .unwrap_or_else(|| "未命名会话".to_string()),
            cwd: record.cwd,
            status: record.status.unwrap_or_default(),
            model: String::new(),
            created_at: record.created_at,
            updated_at: record.updated_at,
            last_activity_at: record.last_activity_at,
            size_bytes: paths.iter().map(|path| path_size(path)).sum(),
        });
    }
    Ok(sessions)
}

#[tauri::command]
pub fn delete_workbuddy_session(session_id: String) -> Result<DeleteSessionResult, String> {
    validate_session_id(&session_id)?;
    let root = workbuddy_dir()?;
    let mut connection = open_database(&root)?;
    let record = load_session(&connection, &session_id)?
        .ok_or_else(|| "会话不存在或已经删除".to_string())?;
    if record.status.as_deref() == Some("working") {
        return Err("正在运行的会话不能删除，请先结束会话".to_string());
    }

    let source_paths = find_session_paths(&root, &session_id)?;
    let deleted_at = Utc::now().timestamp_millis();
    let trash_dir = root
        .join("session-trash")
        .join(deleted_at.to_string())
        .join(&session_id);
    let moved = move_to_trash(&root, &trash_dir, &source_paths, &session_id, deleted_at)?;

    let update_result = (|| -> Result<(), String> {
        let transaction = connection
            .transaction()
            .map_err(|error| format!("开启数据库事务失败：{error}"))?;
        let changed = transaction
            .execute(
                "UPDATE sessions SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2 AND deleted_at IS NULL",
                params![deleted_at, session_id],
            )
            .map_err(|error| format!("软删除会话失败：{error}"))?;
        if changed != 1 {
            return Err("会话状态已变化，未执行删除".to_string());
        }
        transaction
            .commit()
            .map_err(|error| format!("提交会话删除失败：{error}"))
    })();
    if let Err(error) = update_result {
        rollback_moves(&moved);
        return Err(error);
    }

    let index_result = remove_from_recent_index(&root, &session_id);
    Ok(DeleteSessionResult {
        session_id,
        deleted_at,
        trash_dir: trash_dir.to_string_lossy().to_string(),
        moved_items: moved.len(),
        recent_index_updated: index_result.is_ok(),
        warning: index_result.err().map(|error| {
            format!("会话已安全移入回收站并完成软删除，但最近会话索引更新失败：{error}")
        }),
    })
}

fn open_database(root: &Path) -> Result<Connection, String> {
    Connection::open(root.join("workbuddy.db"))
        .map_err(|error| format!("打开 WorkBuddy 会话数据库失败：{error}"))
}

fn map_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRecord> {
    Ok(SessionRecord {
        id: row.get(0)?,
        cwd: row.get(1)?,
        title: row.get(2)?,
        custom_title: row.get(3)?,
        status: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
        last_activity_at: row.get(7)?,
    })
}

fn load_session(connection: &Connection, id: &str) -> Result<Option<SessionRecord>, String> {
    connection
        .query_row(
            "SELECT id, cwd, title, custom_title, status, created_at, updated_at, last_activity_at
             FROM sessions WHERE id = ?1 AND deleted_at IS NULL",
            [id],
            map_session,
        )
        .optional()
        .map_err(|error| format!("查询待删除会话失败：{error}"))
}

fn validate_session_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 200
        || !id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err("无效的会话 ID".to_string());
    }
    Ok(())
}

fn find_session_paths(root: &Path, id: &str) -> Result<Vec<PathBuf>, String> {
    let projects = root.join("projects");
    if !projects.exists() {
        return Ok(Vec::new());
    }
    let file_name = format!("{id}.jsonl");
    let mut paths = Vec::new();
    for entry in WalkDir::new(&projects).min_depth(2).follow_links(false) {
        let entry = entry.map_err(|error| format!("扫描会话文件失败：{error}"))?;
        let path = entry.path();
        if (entry.file_type().is_file() && entry.file_name() == file_name.as_str())
            || (entry.file_type().is_dir() && entry.file_name() == id)
        {
            paths.push(path.to_path_buf());
        }
    }
    paths.sort_by_key(|path| path.components().count());
    paths.dedup();
    let mut independent = Vec::<PathBuf>::new();
    for path in paths {
        if !independent.iter().any(|parent| path.starts_with(parent)) {
            independent.push(path);
        }
    }
    Ok(independent)
}

fn path_size(path: &Path) -> u64 {
    if path.is_file() {
        return fs::metadata(path).map(|metadata| metadata.len()).unwrap_or(0);
    }
    WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| entry.metadata().ok().map(|metadata| metadata.len()))
        .sum()
}

fn move_to_trash(
    root: &Path,
    trash_dir: &Path,
    sources: &[PathBuf],
    session_id: &str,
    deleted_at: i64,
) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    fs::create_dir_all(trash_dir).map_err(|error| format!("创建会话回收站失败：{error}"))?;
    let mut moved = Vec::new();
    for source in sources {
        let relative = source
            .strip_prefix(root)
            .map_err(|_| "会话文件不在 WorkBuddy 数据目录内".to_string())?;
        if relative.components().next().map(|part| part.as_os_str())
            != Some(std::ffi::OsStr::new("projects"))
        {
            rollback_moves(&moved);
            return Err("拒绝移动 projects 目录之外的文件".to_string());
        }
        let destination = trash_dir.join("files").join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("创建回收目录失败：{error}"))?;
        }
        if let Err(error) = fs::rename(source, &destination) {
            rollback_moves(&moved);
            return Err(format!("移动会话文件到回收站失败：{error}"));
        }
        moved.push((source.clone(), destination));
    }
    let metadata = TrashMetadata {
        session_id: session_id.to_string(),
        deleted_at,
        original_paths: sources
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect(),
    };
    let bytes = serde_json::to_vec_pretty(&metadata)
        .map_err(|error| format!("生成回收站元信息失败：{error}"))?;
    if let Err(error) = fs::write(trash_dir.join("metadata.json"), bytes) {
        rollback_moves(&moved);
        return Err(format!("写入回收站元信息失败：{error}"));
    }
    Ok(moved)
}

fn rollback_moves(moved: &[(PathBuf, PathBuf)]) {
    for (source, destination) in moved.iter().rev() {
        if let Some(parent) = source.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::rename(destination, source);
    }
}

fn remove_from_recent_index(root: &Path, session_id: &str) -> Result<(), String> {
    let path = root.join("app").join("sessions.json");
    if !path.exists() {
        return Ok(());
    }
    let bytes = fs::read(&path).map_err(|error| format!("读取 sessions.json 失败：{error}"))?;
    let mut value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("解析 sessions.json 失败：{error}"))?;
    remove_conversation_id(&mut value, session_id);
    let parent = path.parent().ok_or_else(|| "无效的 sessions.json 路径".to_string())?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| format!("创建 sessions.json 临时文件失败：{error}"))?;
    serde_json::to_writer_pretty(temporary.as_file_mut(), &value)
        .map_err(|error| format!("写入 sessions.json 临时文件失败：{error}"))?;
    temporary
        .persist(&path)
        .map_err(|error| format!("安全替换 sessions.json 失败：{}", error.error))?;
    Ok(())
}

fn remove_conversation_id(value: &mut Value, session_id: &str) {
    match value {
        Value::Array(items) => {
            items.retain(|item| {
                item.get("conversationId").and_then(Value::as_str) != Some(session_id)
            });
            for item in items {
                remove_conversation_id(item, session_id);
            }
        }
        Value::Object(object) => {
            for child in object.values_mut() {
                remove_conversation_id(child, session_id);
            }
        }
        _ => {}
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}
