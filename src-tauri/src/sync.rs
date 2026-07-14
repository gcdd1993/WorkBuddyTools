use chrono::Utc;
use rusqlite::{
    backup::Backup, params_from_iter, types::Value as SqlValue, Connection, OptionalExtension,
    Transaction,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};
use walkdir::WalkDir;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

pub use crate::crypto::{decrypt_package, encrypt_package};

const ROOT: &str = "workbuddy-sync";
const MANIFEST_PATH: &str = "workbuddy-sync/manifest.json";
const CHECKSUMS_PATH: &str = "workbuddy-sync/checksums/sha256sums.json";
const SESSIONS_EXPORT_PATH: &str = "workbuddy-sync/sessions/metadata/sessions.export.jsonl";
const TOMBSTONES_PATH: &str = "workbuddy-sync/sessions/tombstones.jsonl";
const MODELS_ARCHIVE_PATH: &str = "workbuddy-sync/models/models.json";
const PROVIDERS_ARCHIVE_PATH: &str = "workbuddy-sync/models/model-providers.json";
const SESSION_PROJECTS_PREFIX: &str = "workbuddy-sync/sessions/projects/";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncStrategy {
    SmartMerge,
    RemoteOverwriteLocal,
    LocalOverwriteRemote,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPackageManifest {
    pub schema_version: u32,
    pub package_format: String,
    pub created_at: String,
    pub files: Vec<SyncPackageFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPackageFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncApplyResult {
    pub backup_dir: Option<PathBuf>,
    pub conflicts: Vec<String>,
}

struct PackageEntry {
    path: String,
    bytes: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionMetadataExport {
    schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default_workspace_path: Option<String>,
    sessions: Vec<BTreeMap<String, Value>>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionTombstone {
    id: String,
    deleted_at: Value,
    updated_at: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionTombstoneExport {
    schema_version: u32,
    tombstones: Vec<SessionTombstone>,
}

#[derive(Debug, Clone)]
struct SessionColumn {
    name: String,
    data_type: String,
    not_null: bool,
    default_value: Option<String>,
    primary_key: bool,
}

#[derive(Debug, Clone)]
struct WorkspacePathRewrite {
    remote: String,
    local: String,
}

pub fn build_sync_package(
    workbuddy_dir: &Path,
    output_zip: &Path,
) -> Result<SyncPackageManifest, String> {
    let mut entries = collect_sync_entries(workbuddy_dir)?;
    let checksums = entries
        .iter()
        .map(|entry| (entry.path.clone(), sha256_hex(&entry.bytes)))
        .collect::<BTreeMap<_, _>>();
    entries.push(PackageEntry {
        path: CHECKSUMS_PATH.to_string(),
        bytes: serde_json::to_vec_pretty(&checksums)
            .map_err(|err| format!("序列化校验清单失败：{err}"))?,
    });

    let mut files = entries
        .iter()
        .map(|entry| SyncPackageFile {
            path: entry.path.clone(),
            size: entry.bytes.len() as u64,
            sha256: sha256_hex(&entry.bytes),
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.path.cmp(&right.path));

    let manifest = SyncPackageManifest {
        schema_version: 1,
        package_format: "workbuddy-webdav-zip-sync".to_string(),
        created_at: Utc::now().to_rfc3339(),
        files,
    };
    let manifest_bytes =
        serde_json::to_vec_pretty(&manifest).map_err(|err| format!("序列化同步清单失败：{err}"))?;
    entries.push(PackageEntry {
        path: MANIFEST_PATH.to_string(),
        bytes: manifest_bytes,
    });
    entries.sort_by(|left, right| left.path.cmp(&right.path));

    if let Some(parent) = output_zip.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("创建 ZIP 输出目录失败：{err}"))?;
    }
    let file = File::create(output_zip).map_err(|err| format!("创建 ZIP 包失败：{err}"))?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o600);
    for entry in entries {
        zip.start_file(entry.path, options)
            .map_err(|err| format!("写入 ZIP 条目失败：{err}"))?;
        zip.write_all(&entry.bytes)
            .map_err(|err| format!("写入 ZIP 内容失败：{err}"))?;
    }
    zip.finish()
        .map_err(|err| format!("完成 ZIP 包失败：{err}"))?;

    Ok(manifest)
}

pub fn inspect_sync_package(zip_path: &Path) -> Result<SyncPackageManifest, String> {
    let mut archive = open_zip(zip_path)?;
    let mut manifest_file = archive
        .by_name(MANIFEST_PATH)
        .map_err(|err| format!("读取同步清单失败：{err}"))?;
    let mut bytes = Vec::new();
    manifest_file
        .read_to_end(&mut bytes)
        .map_err(|err| format!("读取同步清单内容失败：{err}"))?;
    serde_json::from_slice(&bytes).map_err(|err| format!("解析同步清单失败：{err}"))
}

pub fn apply_sync_package(
    workbuddy_dir: &Path,
    zip_path: &Path,
    strategy: SyncStrategy,
) -> Result<SyncApplyResult, String> {
    let entries = read_zip_entries(zip_path)?;
    verify_entries_against_manifest(&entries)?;
    let backup_dir = match strategy {
        SyncStrategy::RemoteOverwriteLocal => Some(create_sync_backup(workbuddy_dir)?),
        SyncStrategy::SmartMerge | SyncStrategy::LocalOverwriteRemote => None,
    };

    apply_model_files(workbuddy_dir, &entries, strategy)?;
    if strategy == SyncStrategy::RemoteOverwriteLocal {
        remove_stale_session_files(workbuddy_dir, &entries)?;
    }
    let conflicts = apply_session_files(workbuddy_dir, &entries, strategy)?;
    apply_session_metadata(workbuddy_dir, &entries, strategy)?;

    Ok(SyncApplyResult {
        backup_dir,
        conflicts,
    })
}

fn remove_stale_session_files(
    workbuddy_dir: &Path,
    entries: &HashMap<String, Vec<u8>>,
) -> Result<(), String> {
    let projects_dir = workbuddy_dir.join("projects");
    if !projects_dir.exists() {
        return Ok(());
    }

    for entry in WalkDir::new(&projects_dir)
        .into_iter()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if !entry.file_type().is_file()
            || path.extension().and_then(|value| value.to_str()) != Some("jsonl")
        {
            continue;
        }
        let relative = path
            .strip_prefix(&projects_dir)
            .map_err(|err| format!("计算本机会话相对路径失败：{err}"))?
            .to_string_lossy()
            .replace('\\', "/");
        let archive_path = format!("{SESSION_PROJECTS_PREFIX}{relative}");
        if !entries.contains_key(&archive_path) {
            fs::remove_file(path).map_err(|err| format!("删除过期本机会话失败：{err}"))?;
        }
    }
    Ok(())
}

pub fn create_sync_backup(workbuddy_dir: &Path) -> Result<PathBuf, String> {
    let backup_dir = workbuddy_dir
        .join("sync-backups")
        .join(Utc::now().format("%Y%m%dT%H%M%SZ").to_string());
    fs::create_dir_all(&backup_dir).map_err(|err| format!("创建同步备份目录失败：{err}"))?;

    for name in ["models.json", "model-providers.json"] {
        let source = workbuddy_dir.join(name);
        if source.exists() {
            fs::copy(&source, backup_dir.join(name))
                .map_err(|err| format!("备份 {name} 失败：{err}"))?;
        }
    }

    let database = workbuddy_dir.join("workbuddy.db");
    if database.exists() {
        let source = Connection::open(&database)
            .map_err(|err| format!("打开待备份的 workbuddy.db 失败：{err}"))?;
        let mut target = Connection::open(backup_dir.join("workbuddy.db"))
            .map_err(|err| format!("创建 workbuddy.db 备份失败：{err}"))?;
        let backup = Backup::new(&source, &mut target)
            .map_err(|err| format!("初始化 workbuddy.db 一致性备份失败：{err}"))?;
        backup
            .run_to_completion(128, std::time::Duration::from_millis(10), None)
            .map_err(|err| format!("备份 workbuddy.db 失败：{err}"))?;
    }

    let projects_dir = workbuddy_dir.join("projects");
    if projects_dir.exists() {
        for entry in WalkDir::new(&projects_dir)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
        {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                continue;
            }
            let relative = path
                .strip_prefix(workbuddy_dir)
                .map_err(|err| format!("计算备份路径失败：{err}"))?;
            let target = backup_dir.join(relative);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|err| format!("创建备份子目录失败：{err}"))?;
            }
            fs::copy(path, &target).map_err(|err| format!("备份会话文件失败：{err}"))?;
        }
    }

    Ok(backup_dir)
}

pub(crate) fn validate_archive_path(name: &str) -> Result<PathBuf, String> {
    let normalized = name.replace('\\', "/");
    if normalized.trim().is_empty() || !normalized.starts_with(&format!("{ROOT}/")) {
        return Err("ZIP 条目必须位于 workbuddy-sync/ 下".to_string());
    }
    if normalized.starts_with('/') || normalized.starts_with('\\') {
        return Err("ZIP 条目不能使用绝对路径".to_string());
    }

    let path = Path::new(&normalized);
    if path.is_absolute() {
        return Err("ZIP 条目不能使用绝对路径".to_string());
    }
    for component in path.components() {
        match component {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("ZIP 条目包含不安全路径".to_string())
            }
            Component::CurDir | Component::Normal(_) => {}
        }
    }
    Ok(path.to_path_buf())
}

pub(crate) fn merge_provider_values(
    local: &Value,
    remote: &Value,
    strategy: SyncStrategy,
) -> Result<Value, String> {
    merge_json_arrays_by_id(
        local,
        remote,
        strategy,
        &["apiKey", "api_key", "token", "secret"],
    )
}

fn merge_model_values(
    local: &Value,
    remote: &Value,
    strategy: SyncStrategy,
) -> Result<Value, String> {
    merge_json_arrays_by_id(
        local,
        remote,
        strategy,
        &["apiKey", "api_key", "token", "secret"],
    )
}

fn collect_sync_entries(workbuddy_dir: &Path) -> Result<Vec<PackageEntry>, String> {
    let mut entries = Vec::new();
    let projects_dir = workbuddy_dir.join("projects");
    if projects_dir.exists() {
        for entry in WalkDir::new(&projects_dir)
            .into_iter()
            .filter_entry(|entry| !is_excluded_component(entry.path()))
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
        {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                continue;
            }
            let relative = path
                .strip_prefix(&projects_dir)
                .map_err(|err| format!("计算会话相对路径失败：{err}"))?;
            let archive_path = format!(
                "{SESSION_PROJECTS_PREFIX}{}",
                path_to_archive_path(relative)?
            );
            entries.push(PackageEntry {
                path: archive_path,
                bytes: fs::read(path).map_err(|err| format!("读取会话文件失败：{err}"))?,
            });
        }
    }

    push_optional_file(
        &mut entries,
        workbuddy_dir.join("models.json"),
        MODELS_ARCHIVE_PATH,
    )?;
    push_optional_file(
        &mut entries,
        workbuddy_dir.join("model-providers.json"),
        PROVIDERS_ARCHIVE_PATH,
    )?;
    let (session_export, tombstones) = export_session_metadata(workbuddy_dir)?;
    entries.push(PackageEntry {
        path: SESSIONS_EXPORT_PATH.to_string(),
        bytes: session_export,
    });
    entries.push(PackageEntry {
        path: TOMBSTONES_PATH.to_string(),
        bytes: tombstones,
    });
    Ok(entries)
}

fn export_session_metadata(workbuddy_dir: &Path) -> Result<(Vec<u8>, Vec<u8>), String> {
    let default_workspace_path = read_default_workspace_path(workbuddy_dir)?;
    let database = workbuddy_dir.join("workbuddy.db");
    if !database.exists() {
        return serialize_session_exports(Vec::new(), Vec::new(), default_workspace_path);
    }
    let connection = Connection::open(&database)
        .map_err(|err| format!("打开 WorkBuddy 会话数据库失败：{err}"))?;
    let columns = session_columns(&connection)?;
    if columns.is_empty() || !columns.iter().any(|column| column.name == "id") {
        return Err("WorkBuddy sessions 表缺少 id 列".to_string());
    }
    let names = columns
        .iter()
        .map(|column| quote_identifier(&column.name))
        .collect::<Vec<_>>();
    let has_deleted_at = columns.iter().any(|column| column.name == "deleted_at");
    let has_status = columns.iter().any(|column| column.name == "status");
    let active_filter = match (has_deleted_at, has_status) {
        (true, true) => " WHERE deleted_at IS NULL AND COALESCE(status, '') <> 'working'",
        (true, false) => " WHERE deleted_at IS NULL",
        (false, true) => " WHERE COALESCE(status, '') <> 'working'",
        (false, false) => "",
    };
    let mut active = Vec::new();
    let mut statement = connection
        .prepare(&format!(
            "SELECT {} FROM sessions{active_filter}",
            names.join(", ")
        ))
        .map_err(|err| format!("准备会话元数据导出失败：{err}"))?;
    let rows = statement
        .query_map([], |row| {
            let mut record = BTreeMap::new();
            for (index, column) in columns.iter().enumerate() {
                record.insert(
                    column.name.clone(),
                    sql_to_json(row.get::<_, SqlValue>(index)?)?,
                );
            }
            Ok(record)
        })
        .map_err(|err| format!("查询会话元数据失败：{err}"))?;
    for row in rows {
        active.push(row.map_err(|err| format!("读取会话元数据失败：{err}"))?);
    }

    let mut deleted = Vec::new();
    if has_deleted_at {
        let updated_expression = if columns.iter().any(|column| column.name == "updated_at") {
            "updated_at"
        } else {
            "NULL"
        };
        let deleted_filter = if has_status {
            " WHERE deleted_at IS NOT NULL AND COALESCE(status, '') <> 'working'"
        } else {
            " WHERE deleted_at IS NOT NULL"
        };
        let mut statement = connection
            .prepare(&format!(
                "SELECT id, deleted_at, {updated_expression} FROM sessions{deleted_filter}"
            ))
            .map_err(|err| format!("准备会话墓碑导出失败：{err}"))?;
        let rows = statement
            .query_map([], |row| {
                let updated_at = sql_to_json(row.get::<_, SqlValue>(2)?)?;
                Ok(SessionTombstone {
                    id: row.get(0)?,
                    deleted_at: sql_to_json(row.get::<_, SqlValue>(1)?)?,
                    updated_at: (!updated_at.is_null()).then_some(updated_at),
                })
            })
            .map_err(|err| format!("查询会话墓碑失败：{err}"))?;
        for row in rows {
            deleted.push(row.map_err(|err| format!("读取会话墓碑失败：{err}"))?);
        }
    }
    serialize_session_exports(active, deleted, default_workspace_path)
}

fn serialize_session_exports(
    sessions: Vec<BTreeMap<String, Value>>,
    tombstones: Vec<SessionTombstone>,
    default_workspace_path: Option<String>,
) -> Result<(Vec<u8>, Vec<u8>), String> {
    let sessions = serde_json::to_vec(&SessionMetadataExport {
        schema_version: 1,
        default_workspace_path,
        sessions,
    })
    .map_err(|err| format!("序列化会话元数据失败：{err}"))?;
    let tombstones = serde_json::to_vec(&SessionTombstoneExport {
        schema_version: 1,
        tombstones,
    })
    .map_err(|err| format!("序列化会话墓碑失败：{err}"))?;
    Ok((sessions, tombstones))
}

fn sql_to_json(value: SqlValue) -> rusqlite::Result<Value> {
    Ok(match value {
        SqlValue::Null => Value::Null,
        SqlValue::Integer(value) => Value::from(value),
        SqlValue::Real(value) => Value::from(value),
        SqlValue::Text(value) => Value::String(value),
        SqlValue::Blob(_) => return Err(rusqlite::Error::InvalidQuery),
    })
}

fn json_to_sql(value: &Value) -> Result<SqlValue, String> {
    match value {
        Value::Null => Ok(SqlValue::Null),
        Value::Bool(_) | Value::Array(_) | Value::Object(_) => {
            Err("会话元数据包含 SQLite 不支持的 JSON 值".to_string())
        }
        Value::Number(value) => value
            .as_i64()
            .map(SqlValue::Integer)
            .or_else(|| value.as_f64().map(SqlValue::Real))
            .ok_or_else(|| "会话元数据包含无效数值".to_string()),
        Value::String(value) => Ok(SqlValue::Text(value.clone())),
    }
}

fn push_optional_file(
    entries: &mut Vec<PackageEntry>,
    source: PathBuf,
    archive_path: &str,
) -> Result<(), String> {
    if source.exists() {
        entries.push(PackageEntry {
            path: archive_path.to_string(),
            bytes: fs::read(&source)
                .map_err(|err| format!("读取 {} 失败：{err}", source.display()))?,
        });
    }
    Ok(())
}

fn read_default_workspace_path(workbuddy_dir: &Path) -> Result<Option<String>, String> {
    let config_path = workbuddy_dir.join("app").join("app-config.json");
    if !config_path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&config_path)
        .map_err(|err| format!("读取 WorkBuddy 默认工作空间配置失败：{err}"))?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|err| format!("解析 WorkBuddy 默认工作空间配置失败：{err}"))?;
    Ok(value
        .get("defaultWorkspacePath")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned))
}

fn is_excluded_component(path: &Path) -> bool {
    path.components().any(|component| {
        let text = component.as_os_str().to_string_lossy();
        matches!(
            text.as_ref(),
            "sessions" | "session" | "cache" | "credentials" | "logs" | "target" | "sync-backups"
        )
    })
}

fn path_to_archive_path(path: &Path) -> Result<String, String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => parts.push(value.to_string_lossy().to_string()),
            _ => return Err("路径包含不支持的组件".to_string()),
        }
    }
    Ok(parts.join("/"))
}

fn open_zip(zip_path: &Path) -> Result<ZipArchive<File>, String> {
    let file = File::open(zip_path).map_err(|err| format!("打开 ZIP 包失败：{err}"))?;
    ZipArchive::new(file).map_err(|err| format!("解析 ZIP 包失败：{err}"))
}

fn read_zip_entries(zip_path: &Path) -> Result<HashMap<String, Vec<u8>>, String> {
    let mut archive = open_zip(zip_path)?;
    let mut entries = HashMap::new();
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|err| format!("读取 ZIP 条目失败：{err}"))?;
        if file.is_dir() {
            continue;
        }
        let name = file.name().replace('\\', "/");
        validate_archive_path(&name)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|err| format!("读取 ZIP 条目内容失败：{err}"))?;
        entries.insert(name, bytes);
    }
    Ok(entries)
}

fn verify_entries_against_manifest(entries: &HashMap<String, Vec<u8>>) -> Result<(), String> {
    let manifest_bytes = entries
        .get(MANIFEST_PATH)
        .ok_or_else(|| "同步 ZIP 缺少 manifest.json".to_string())?;
    let manifest: SyncPackageManifest =
        serde_json::from_slice(manifest_bytes).map_err(|err| format!("解析同步清单失败：{err}"))?;

    for file in manifest.files {
        let bytes = entries
            .get(&file.path)
            .ok_or_else(|| format!("同步 ZIP 缺少清单文件：{}", file.path))?;
        if bytes.len() as u64 != file.size {
            return Err(format!(
                "同步 ZIP 文件大小校验失败：{} expected {} got {}",
                file.path,
                file.size,
                bytes.len()
            ));
        }
        let actual_hash = sha256_hex(bytes);
        if actual_hash != file.sha256 {
            return Err(format!(
                "同步 ZIP hash 校验失败：{} expected {} got {}",
                file.path, file.sha256, actual_hash
            ));
        }
    }
    Ok(())
}

fn apply_model_files(
    workbuddy_dir: &Path,
    entries: &HashMap<String, Vec<u8>>,
    strategy: SyncStrategy,
) -> Result<(), String> {
    apply_json_array_file(
        &workbuddy_dir.join("models.json"),
        entries.get(MODELS_ARCHIVE_PATH),
        strategy,
        merge_model_values,
    )?;
    apply_json_array_file(
        &workbuddy_dir.join("model-providers.json"),
        entries.get(PROVIDERS_ARCHIVE_PATH),
        strategy,
        merge_provider_values,
    )
}

fn apply_json_array_file(
    target: &Path,
    remote_bytes: Option<&Vec<u8>>,
    strategy: SyncStrategy,
    merge: fn(&Value, &Value, SyncStrategy) -> Result<Value, String>,
) -> Result<(), String> {
    let Some(remote_bytes) = remote_bytes else {
        return Ok(());
    };
    let remote_value: Value = serde_json::from_slice(remote_bytes)
        .map_err(|err| format!("解析远端 JSON 配置失败：{err}"))?;

    let next = match strategy {
        SyncStrategy::RemoteOverwriteLocal => remote_value,
        SyncStrategy::LocalOverwriteRemote => read_json_array_or_empty(target)?,
        SyncStrategy::SmartMerge => {
            let local_value = read_json_array_or_empty(target)?;
            merge(&local_value, &remote_value, strategy)?
        }
    };
    write_json_value(target, &next)
}

fn apply_session_metadata(
    workbuddy_dir: &Path,
    entries: &HashMap<String, Vec<u8>>,
    strategy: SyncStrategy,
) -> Result<(), String> {
    if strategy == SyncStrategy::LocalOverwriteRemote {
        return Ok(());
    }
    let Some(export_bytes) = entries.get(SESSIONS_EXPORT_PATH) else {
        return Ok(());
    };
    let export: SessionMetadataExport =
        serde_json::from_slice(export_bytes).map_err(|err| format!("解析会话元数据失败：{err}"))?;
    if export.schema_version != 1 {
        return Err("不支持的会话元数据版本".to_string());
    }
    let tombstone_export = entries
        .get(TOMBSTONES_PATH)
        .map(|bytes| {
            serde_json::from_slice::<SessionTombstoneExport>(bytes)
                .map_err(|err| format!("解析会话墓碑失败：{err}"))
        })
        .transpose()?
        .unwrap_or(SessionTombstoneExport {
            schema_version: 1,
            tombstones: Vec::new(),
        });
    if tombstone_export.schema_version != 1 {
        return Err("不支持的会话墓碑版本".to_string());
    }
    let local_default_workspace_path = read_default_workspace_path(workbuddy_dir)?;
    let workspace_rewrite = workspace_path_rewrite(
        export.default_workspace_path.as_deref(),
        local_default_workspace_path.as_deref(),
    );

    let database = workbuddy_dir.join("workbuddy.db");
    if !database.exists() {
        if export.sessions.is_empty() && tombstone_export.tombstones.is_empty() {
            return Ok(());
        }
        return Err("本机缺少 workbuddy.db，无法导入会话元数据".to_string());
    }
    let mut connection = Connection::open(&database)
        .map_err(|err| format!("打开 WorkBuddy 会话数据库失败：{err}"))?;
    let columns = session_columns(&connection)?;
    let column_map = columns
        .iter()
        .map(|column| (column.name.as_str(), column))
        .collect::<HashMap<_, _>>();
    if !column_map.contains_key("id") {
        return Err("WorkBuddy sessions 表缺少 id 列".to_string());
    }
    let transaction = connection
        .transaction()
        .map_err(|err| format!("开启会话元数据事务失败：{err}"))?;
    let local_user_id = select_local_user_id(&transaction, &column_map)?;
    let user_id_required = column_map
        .get("user_id")
        .is_some_and(|column| column.not_null && column.default_value.is_none());
    if user_id_required && local_user_id.is_none() && !export.sessions.is_empty() {
        return Err("无法确定本机用户，不能导入会话元数据".to_string());
    }
    let remote_ids = export
        .sessions
        .iter()
        .map(|record| {
            record_text(record, "id").ok_or_else(|| "远端会话元数据缺少文本 id".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;

    for record in &export.sessions {
        upsert_session_record(
            &transaction,
            &columns,
            &column_map,
            record,
            strategy,
            local_user_id.as_deref(),
        )?;
    }
    if strategy == SyncStrategy::RemoteOverwriteLocal {
        soft_delete_missing_sessions(&transaction, &columns, &remote_ids)?;
    }
    for tombstone in &tombstone_export.tombstones {
        apply_session_tombstone(&transaction, &columns, tombstone, strategy)?;
    }
    if let Some(rewrite) = workspace_rewrite.as_ref() {
        rewrite_workspace_paths(&transaction, rewrite)?;
    }
    transaction
        .commit()
        .map_err(|err| format!("提交会话元数据事务失败：{err}"))
}

fn select_local_user_id(
    transaction: &Transaction<'_>,
    column_map: &HashMap<&str, &SessionColumn>,
) -> Result<Option<String>, String> {
    if !column_map.contains_key("user_id") {
        return Ok(None);
    }
    let active_order = if column_map.contains_key("deleted_at") {
        "CASE WHEN deleted_at IS NULL THEN 0 ELSE 1 END"
    } else {
        "0"
    };
    let recent_order = match (
        column_map.contains_key("updated_at"),
        column_map.contains_key("created_at"),
    ) {
        (true, true) => "CAST(COALESCE(updated_at, created_at, 0) AS INTEGER)",
        (true, false) => "CAST(COALESCE(updated_at, 0) AS INTEGER)",
        (false, true) => "CAST(COALESCE(created_at, 0) AS INTEGER)",
        (false, false) => "0",
    };
    let sql = format!(
        "SELECT CAST(user_id AS TEXT) FROM sessions \
         WHERE user_id IS NOT NULL AND TRIM(CAST(user_id AS TEXT)) <> '' \
         ORDER BY {active_order} ASC, {recent_order} DESC LIMIT 1"
    );
    transaction
        .query_row(&sql, [], |row| row.get(0))
        .optional()
        .map_err(|err| format!("确定本机用户失败：{err}"))
}

fn session_columns(connection: &Connection) -> Result<Vec<SessionColumn>, String> {
    let mut statement = connection
        .prepare("PRAGMA table_info(sessions)")
        .map_err(|err| format!("读取 sessions 表结构失败：{err}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok(SessionColumn {
                name: row.get(1)?,
                data_type: row.get(2)?,
                not_null: row.get::<_, i64>(3)? != 0,
                default_value: row.get(4)?,
                primary_key: row.get::<_, i64>(5)? != 0,
            })
        })
        .map_err(|err| format!("查询 sessions 表结构失败：{err}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("解析 sessions 表结构失败：{err}"))
}

fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn record_text(record: &BTreeMap<String, Value>, column: &str) -> Option<String> {
    match record.get(column) {
        Some(Value::String(value)) if !value.is_empty() => Some(value.clone()),
        _ => None,
    }
}

fn workspace_path_rewrite(
    remote: Option<&str>,
    local: Option<&str>,
) -> Option<WorkspacePathRewrite> {
    let remote = trim_trailing_path_separators(remote?).to_string();
    let local = trim_trailing_path_separators(local?).to_string();
    if remote.is_empty()
        || local.is_empty()
        || canonical_workspace_path(&remote) == canonical_workspace_path(&local)
    {
        return None;
    }
    Some(WorkspacePathRewrite { remote, local })
}

fn rewrite_workspace_paths(
    transaction: &Transaction<'_>,
    rewrite: &WorkspacePathRewrite,
) -> Result<(), String> {
    rewrite_table_path_column(transaction, "sessions", "cwd", rewrite)?;
    rewrite_table_path_column(transaction, "workspaces", "path", rewrite)
}

fn rewrite_table_path_column(
    transaction: &Transaction<'_>,
    table: &str,
    column: &str,
    rewrite: &WorkspacePathRewrite,
) -> Result<(), String> {
    if !sqlite_table_exists(transaction, table)?
        || !sqlite_column_exists(transaction, table, column)?
    {
        return Ok(());
    }

    let table_identifier = quote_identifier(table);
    let column_identifier = quote_identifier(column);
    let replacements = {
        let mut statement = transaction
            .prepare(&format!(
                "SELECT DISTINCT {column_identifier} FROM {table_identifier} WHERE {column_identifier} IS NOT NULL"
            ))
            .map_err(|err| format!("准备修复 {table}.{column} 路径失败：{err}"))?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|err| format!("查询待修复 {table}.{column} 路径失败：{err}"))?;
        let mut replacements = Vec::new();
        for row in rows {
            let old_path =
                row.map_err(|err| format!("读取待修复 {table}.{column} 路径失败：{err}"))?;
            if let Some(new_path) = rewrite_workspace_path(&old_path, rewrite) {
                if new_path != old_path {
                    replacements.push((old_path, new_path));
                }
            }
        }
        replacements
    };

    if replacements.is_empty() {
        return Ok(());
    }
    let sql = format!(
        "UPDATE {table_identifier} SET {column_identifier} = ?1 WHERE {column_identifier} = ?2"
    );
    for (old_path, new_path) in replacements {
        transaction
            .execute(&sql, rusqlite::params![new_path, old_path])
            .map_err(|err| format!("修复 {table}.{column} 路径失败：{err}"))?;
    }
    Ok(())
}

fn sqlite_table_exists(transaction: &Transaction<'_>, table: &str) -> Result<bool, String> {
    transaction
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1 LIMIT 1",
            [table],
            |_| Ok(()),
        )
        .optional()
        .map(|value| value.is_some())
        .map_err(|err| format!("检查数据库表 {table} 失败：{err}"))
}

fn sqlite_column_exists(
    transaction: &Transaction<'_>,
    table: &str,
    column: &str,
) -> Result<bool, String> {
    let mut statement = transaction
        .prepare(&format!("PRAGMA table_info({})", quote_identifier(table)))
        .map_err(|err| format!("读取数据库表 {table} 结构失败：{err}"))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|err| format!("查询数据库表 {table} 结构失败：{err}"))?;
    for row in rows {
        if row.map_err(|err| format!("解析数据库表 {table} 结构失败：{err}"))? == column
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn rewrite_workspace_path(value: &str, rewrite: &WorkspacePathRewrite) -> Option<String> {
    let prefix_len = workspace_prefix_len(value, &rewrite.remote)?;
    let suffix = &value[prefix_len..];
    let local = trim_trailing_path_separators(&rewrite.local);
    let suffix = if path_ends_with_separator(local) && suffix.starts_with(is_path_separator) {
        &suffix[1..]
    } else {
        suffix
    };
    Some(format!("{local}{suffix}"))
}

fn workspace_prefix_len(value: &str, prefix: &str) -> Option<usize> {
    let prefix = trim_trailing_path_separators(prefix);
    let mut value_chars = value.char_indices();
    let mut last_end = 0;
    for prefix_char in prefix.chars() {
        let (index, value_char) = value_chars.next()?;
        if normalize_path_char(prefix_char) != normalize_path_char(value_char) {
            return None;
        }
        last_end = index + value_char.len_utf8();
    }
    let suffix = &value[last_end..];
    if suffix.is_empty() || suffix.starts_with(is_path_separator) {
        Some(last_end)
    } else {
        None
    }
}

fn trim_trailing_path_separators(value: &str) -> &str {
    let trimmed = value.trim();
    let mut end = trimmed.len();
    while end > 0 {
        let Some(character) = trimmed[..end].chars().next_back() else {
            break;
        };
        if !is_path_separator(character) {
            break;
        }
        let next_end = end - character.len_utf8();
        let without_separator = &trimmed[..next_end];
        if without_separator.is_empty() || without_separator.ends_with(':') {
            break;
        }
        end = next_end;
    }
    &trimmed[..end]
}

fn canonical_workspace_path(value: &str) -> String {
    trim_trailing_path_separators(value)
        .chars()
        .map(normalize_path_char)
        .collect()
}

fn normalize_path_char(character: char) -> char {
    if is_path_separator(character) {
        '/'
    } else {
        character.to_ascii_lowercase()
    }
}

fn path_ends_with_separator(value: &str) -> bool {
    value.chars().next_back().is_some_and(is_path_separator)
}

fn is_path_separator(character: char) -> bool {
    matches!(character, '\\' | '/')
}

fn upsert_session_record(
    transaction: &Transaction<'_>,
    columns: &[SessionColumn],
    column_map: &HashMap<&str, &SessionColumn>,
    remote: &BTreeMap<String, Value>,
    strategy: SyncStrategy,
    local_user_id: Option<&str>,
) -> Result<(), String> {
    let id = record_text(remote, "id").ok_or_else(|| "远端会话元数据缺少文本 id".to_string())?;
    let has_status = column_map.contains_key("status");
    let has_updated = column_map.contains_key("updated_at");
    let selected_columns = [
        has_status.then_some("status"),
        has_updated.then_some("updated_at"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    let local = transaction
        .query_row(
            &format!(
                "SELECT {} FROM sessions WHERE id = ?1",
                if selected_columns.is_empty() {
                    "1".to_string()
                } else {
                    selected_columns.join(", ")
                }
            ),
            [&id],
            |row| {
                let mut index = 0;
                let status = if has_status {
                    let value = row.get::<_, SqlValue>(index)?;
                    index += 1;
                    Some(value)
                } else {
                    None
                };
                let updated = if has_updated {
                    Some(row.get::<_, SqlValue>(index)?)
                } else {
                    None
                };
                Ok((status, updated))
            },
        )
        .optional()
        .map_err(|err| format!("查询本机会话 {id} 失败：{err}"))?;

    if let Some((status, updated)) = &local {
        if matches!(status, Some(SqlValue::Text(value)) if value == "working") {
            return Ok(());
        }
        if strategy == SyncStrategy::SmartMerge
            && !is_remote_newer(remote.get("updated_at"), updated.as_ref())
        {
            return Ok(());
        }
        let update_columns = columns
            .iter()
            .filter(|column| {
                column.name != "id" && column.name != "user_id" && remote.contains_key(&column.name)
            })
            .collect::<Vec<_>>();
        if update_columns.is_empty() {
            return Ok(());
        }
        let sql = format!(
            "UPDATE sessions SET {} WHERE id = ?",
            update_columns
                .iter()
                .map(|column| format!("{} = ?", quote_identifier(&column.name)))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let mut values = update_columns
            .iter()
            .map(|column| json_to_sql(&remote[&column.name]))
            .collect::<Result<Vec<_>, _>>()?;
        values.push(SqlValue::Text(id));
        transaction
            .execute(&sql, params_from_iter(values))
            .map_err(|err| format!("更新会话元数据失败：{err}"))?;
        return Ok(());
    }

    let primary_keys = columns
        .iter()
        .filter(|column| column.primary_key)
        .collect::<Vec<_>>();
    let auto_generated_primary_key =
        primary_keys.len() == 1 && primary_keys[0].data_type.eq_ignore_ascii_case("INTEGER");
    for column in columns {
        let may_be_generated = auto_generated_primary_key && column.primary_key;
        if (column.not_null || column.primary_key)
            && !may_be_generated
            && column.default_value.is_none()
            && !(column.name == "user_id" && local_user_id.is_some())
            && !remote.contains_key(&column.name)
        {
            return Err(format!(
                "远端会话 {id} 缺少目标数据库必填列 {}",
                column.name
            ));
        }
    }
    let insert_columns = columns
        .iter()
        .filter(|column| {
            if column.name == "user_id" {
                local_user_id.is_some()
            } else {
                remote.contains_key(&column.name)
            }
        })
        .collect::<Vec<_>>();
    let sql = format!(
        "INSERT INTO sessions ({}) VALUES ({})",
        insert_columns
            .iter()
            .map(|column| quote_identifier(&column.name))
            .collect::<Vec<_>>()
            .join(", "),
        vec!["?"; insert_columns.len()].join(", ")
    );
    let values = insert_columns
        .iter()
        .map(|column| {
            if column.name == "user_id" {
                Ok(SqlValue::Text(
                    local_user_id
                        .expect("user_id column requires local user")
                        .to_string(),
                ))
            } else {
                json_to_sql(&remote[&column.name])
            }
        })
        .collect::<Result<Vec<_>, String>>()?;
    transaction
        .execute(&sql, params_from_iter(values))
        .map_err(|err| format!("插入会话元数据失败：{err}"))?;
    Ok(())
}

fn soft_delete_missing_sessions(
    transaction: &Transaction<'_>,
    columns: &[SessionColumn],
    remote_ids: &[String],
) -> Result<(), String> {
    if !columns.iter().any(|column| column.name == "deleted_at") {
        return Err("WorkBuddy sessions 表缺少 deleted_at 列，无法执行远端覆盖".to_string());
    }

    let now = Utc::now().timestamp_millis();
    let mut sql = "UPDATE sessions SET deleted_at = ?".to_string();
    let has_updated_at = columns.iter().any(|column| column.name == "updated_at");
    if has_updated_at {
        sql.push_str(", updated_at = ?");
    }
    sql.push_str(" WHERE deleted_at IS NULL");
    if columns.iter().any(|column| column.name == "status") {
        sql.push_str(" AND COALESCE(status, '') <> 'working'");
    }

    let mut values = vec![SqlValue::Integer(now)];
    if has_updated_at {
        values.push(SqlValue::Integer(now));
    }
    if !remote_ids.is_empty() {
        sql.push_str(&format!(
            " AND id NOT IN ({})",
            vec!["?"; remote_ids.len()].join(", ")
        ));
        values.extend(remote_ids.iter().cloned().map(SqlValue::Text));
    }
    transaction
        .execute(&sql, params_from_iter(values))
        .map_err(|err| format!("软删除远端不存在的本机会话失败：{err}"))?;
    Ok(())
}

fn apply_session_tombstone(
    transaction: &Transaction<'_>,
    columns: &[SessionColumn],
    tombstone: &SessionTombstone,
    strategy: SyncStrategy,
) -> Result<(), String> {
    if tombstone.id.trim().is_empty() {
        return Err("远端会话墓碑缺少 id".to_string());
    }
    if !columns.iter().any(|column| column.name == "deleted_at") {
        return Ok(());
    }

    let has_status = columns.iter().any(|column| column.name == "status");
    let has_updated_at = columns.iter().any(|column| column.name == "updated_at");
    let selected = [
        has_status.then_some("status"),
        has_updated_at.then_some("updated_at"),
        Some("deleted_at"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    let local = transaction
        .query_row(
            &format!("SELECT {} FROM sessions WHERE id = ?1", selected.join(", ")),
            [&tombstone.id],
            |row| {
                let mut index = 0;
                let status = if has_status {
                    let value = row.get::<_, SqlValue>(index)?;
                    index += 1;
                    Some(value)
                } else {
                    None
                };
                let updated_at = if has_updated_at {
                    let value = row.get::<_, SqlValue>(index)?;
                    index += 1;
                    Some(value)
                } else {
                    None
                };
                let deleted_at = row.get::<_, SqlValue>(index)?;
                Ok((status, updated_at, deleted_at))
            },
        )
        .optional()
        .map_err(|err| format!("查询墓碑对应会话 {} 失败：{err}", tombstone.id))?;
    let Some((status, local_updated_at, local_deleted_at)) = local else {
        return Ok(());
    };
    if matches!(status, Some(SqlValue::Text(value)) if value == "working") {
        return Ok(());
    }

    let remote_clock = tombstone
        .updated_at
        .as_ref()
        .filter(|value| !value.is_null())
        .unwrap_or(&tombstone.deleted_at);
    let local_clock = if !matches!(local_deleted_at, SqlValue::Null) {
        &local_deleted_at
    } else if let Some(value) = local_updated_at.as_ref() {
        value
    } else {
        &SqlValue::Null
    };
    if strategy == SyncStrategy::SmartMerge
        && !is_remote_newer(Some(remote_clock), Some(local_clock))
        && !matches!(local_clock, SqlValue::Null)
    {
        return Ok(());
    }

    let mut assignments = vec!["deleted_at = ?"];
    let mut values = vec![json_to_sql(&tombstone.deleted_at)?];
    if has_updated_at {
        assignments.push("updated_at = ?");
        values.push(json_to_sql(remote_clock)?);
    }
    values.push(SqlValue::Text(tombstone.id.clone()));
    transaction
        .execute(
            &format!(
                "UPDATE sessions SET {} WHERE id = ?",
                assignments.join(", ")
            ),
            params_from_iter(values),
        )
        .map_err(|err| format!("应用会话墓碑 {} 失败：{err}", tombstone.id))?;
    Ok(())
}

fn is_remote_newer(remote: Option<&Value>, local: Option<&SqlValue>) -> bool {
    timestamp_number_export(remote)
        .zip(timestamp_number_sql(local))
        .map(|(remote, local)| remote > local)
        .unwrap_or(false)
}

fn timestamp_number_export(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(v) => v.as_f64(),
        Value::String(v) => v.parse().ok(),
        _ => None,
    }
}

fn timestamp_number_sql(value: Option<&SqlValue>) -> Option<f64> {
    match value? {
        SqlValue::Integer(v) => Some(*v as f64),
        SqlValue::Real(v) => Some(*v),
        SqlValue::Text(v) => v.parse().ok(),
        _ => None,
    }
}

fn apply_session_files(
    workbuddy_dir: &Path,
    entries: &HashMap<String, Vec<u8>>,
    strategy: SyncStrategy,
) -> Result<Vec<String>, String> {
    let mut conflicts = Vec::new();

    for (archive_path, remote_bytes) in entries {
        let Some(relative) = archive_path.strip_prefix(SESSION_PROJECTS_PREFIX) else {
            continue;
        };
        let target = workbuddy_dir
            .join("projects")
            .join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
        if strategy == SyncStrategy::SmartMerge && target.exists() {
            let local_bytes =
                fs::read(&target).map_err(|err| format!("读取本机会话失败：{err}"))?;
            if local_bytes == *remote_bytes || local_bytes.starts_with(remote_bytes) {
                continue;
            }
            if remote_bytes.starts_with(&local_bytes) {
                write_bytes(&target, remote_bytes)?;
                continue;
            }
            let conflict = conflict_path(&target);
            write_bytes(&conflict, remote_bytes)?;
            conflicts.push(conflict.to_string_lossy().to_string());
            continue;
        }
        write_bytes(&target, remote_bytes)?;
    }
    Ok(conflicts)
}

fn conflict_path(target: &Path) -> PathBuf {
    let timestamp = Utc::now().format("%Y%m%dT%H%M%SZ");
    let stem = target
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("session");
    target.with_file_name(format!("{stem}.conflict.{timestamp}.jsonl"))
}

fn merge_json_arrays_by_id(
    local: &Value,
    remote: &Value,
    strategy: SyncStrategy,
    secret_fields: &[&str],
) -> Result<Value, String> {
    if strategy == SyncStrategy::RemoteOverwriteLocal {
        return Ok(remote.clone());
    }
    if strategy == SyncStrategy::LocalOverwriteRemote {
        return Ok(local.clone());
    }

    let mut merged = Vec::<Value>::new();
    let mut indexes = HashMap::<String, usize>::new();
    for item in local
        .as_array()
        .ok_or_else(|| "本地 JSON 配置不是数组".to_string())?
    {
        if let Some(id) = value_id(item) {
            indexes.insert(id, merged.len());
        }
        merged.push(item.clone());
    }

    for remote_item in remote
        .as_array()
        .ok_or_else(|| "远端 JSON 配置不是数组".to_string())?
    {
        let Some(id) = value_id(remote_item) else {
            merged.push(remote_item.clone());
            continue;
        };
        if let Some(index) = indexes.get(&id).copied() {
            let local_item = merged[index].clone();
            merged[index] =
                merge_json_object_preserving_secrets(&local_item, remote_item, secret_fields)?;
        } else {
            indexes.insert(id, merged.len());
            merged.push(remote_item.clone());
        }
    }

    Ok(Value::Array(merged))
}

fn merge_json_object_preserving_secrets(
    local: &Value,
    remote: &Value,
    secret_fields: &[&str],
) -> Result<Value, String> {
    let mut output = remote
        .as_object()
        .cloned()
        .ok_or_else(|| "远端 JSON 条目不是对象".to_string())?;
    let local_object = local
        .as_object()
        .ok_or_else(|| "本地 JSON 条目不是对象".to_string())?;
    for field in secret_fields {
        if let Some(local_value) = local_object.get(*field) {
            output.insert((*field).to_string(), local_value.clone());
        }
    }
    Ok(Value::Object(output))
}

fn value_id(value: &Value) -> Option<String> {
    value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .map(ToString::to_string)
}

fn read_json_array_or_empty(path: &Path) -> Result<Value, String> {
    if !path.exists() {
        return Ok(Value::Array(Vec::new()));
    }
    let content = fs::read_to_string(path).map_err(|err| format!("读取 JSON 配置失败：{err}"))?;
    if content.trim().is_empty() {
        return Ok(Value::Array(Vec::new()));
    }
    serde_json::from_str(&content).map_err(|err| format!("解析 JSON 配置失败：{err}"))
}

fn write_json_value(path: &Path, value: &Value) -> Result<(), String> {
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|err| format!("序列化 JSON 配置失败：{err}"))?;
    let mut with_newline = bytes;
    with_newline.push(b'\n');
    write_bytes(path, &with_newline)
}

fn write_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("创建输出目录失败：{err}"))?;
    }
    fs::write(path, bytes).map_err(|err| format!("写入文件失败：{err}"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "workbuddy-sync-{name}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("create temp root");
        root
    }

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, content).expect("write file");
    }

    fn write_app_config(root: &Path, default_workspace_path: &str) {
        let content = serde_json::to_string(&json!({
            "defaultWorkspacePath": default_workspace_path
        }))
        .expect("serialize app config");
        write(&root.join("app").join("app-config.json"), &content);
    }

    fn create_sync_database(root: &Path, session_cwd: Option<&str>, workspace_path: Option<&str>) {
        let connection = Connection::open(root.join("workbuddy.db")).expect("open test db");
        connection
            .execute_batch(
                "CREATE TABLE sessions (
                    id TEXT PRIMARY KEY,
                    cwd TEXT,
                    title TEXT,
                    status TEXT,
                    updated_at INTEGER,
                    deleted_at INTEGER
                );
                CREATE TABLE workspaces (
                    id TEXT PRIMARY KEY,
                    path TEXT
                );",
            )
            .expect("create test schema");
        if let Some(cwd) = session_cwd {
            connection
                .execute(
                    "INSERT INTO sessions (id, cwd, title, status, updated_at, deleted_at)
                     VALUES (?1, ?2, 'Remote Session', 'idle', 200, NULL)",
                    rusqlite::params!["session-1", cwd],
                )
                .expect("insert test session");
        }
        if let Some(path) = workspace_path {
            connection
                .execute(
                    "INSERT INTO workspaces (id, path) VALUES ('workspace-1', ?1)",
                    [path],
                )
                .expect("insert test workspace");
        }
    }

    #[test]
    fn package_includes_sessions_models_providers_and_excludes_runtime_files() {
        let root = temp_root("package");
        write(
            &root.join("projects/cwd-key/session-1.jsonl"),
            "{\"type\":\"message\"}\n",
        );
        write(&root.join("sessions/12345.json"), "{\"pid\":12345}");
        write(&root.join("app/session/Cache/blob"), "cache");
        write(&root.join("app/sessions.json"), "[]");
        write(
            &root.join("models.json"),
            r#"[{"id":"model-a","apiKey":"sk-model"}]"#,
        );
        write(
            &root.join("model-providers.json"),
            r#"[{"id":"provider-a","apiKey":"sk-provider"}]"#,
        );

        let package = root.join("package.zip");
        let manifest = build_sync_package(&root, &package).expect("build package");
        let paths = manifest
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>();

        assert!(paths.contains(&"workbuddy-sync/sessions/projects/cwd-key/session-1.jsonl"));
        assert!(paths.contains(&"workbuddy-sync/models/models.json"));
        assert!(paths.contains(&"workbuddy-sync/models/model-providers.json"));
        assert!(paths.contains(&"workbuddy-sync/sessions/metadata/sessions.export.jsonl"));
        assert!(!paths
            .iter()
            .any(|path| path.contains("sessions/12345.json")));
        assert!(!paths.iter().any(|path| path.contains("app/session")));
        assert!(!paths.iter().any(|path| path.ends_with("app/sessions.json")));

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn safe_extract_rejects_zip_slip_paths() {
        assert!(validate_archive_path("workbuddy-sync/models/models.json").is_ok());
        assert!(validate_archive_path("../evil.txt").is_err());
        assert!(validate_archive_path("workbuddy-sync/../../evil.txt").is_err());
        assert!(validate_archive_path("C:/Users/evil.txt").is_err());
        assert!(validate_archive_path("/tmp/evil.txt").is_err());
    }

    #[test]
    fn smart_merge_keeps_local_provider_api_key_when_remote_differs() {
        let local = json!([
            {"id":"provider-a","name":"Local","baseUrl":"https://local.example/v1","apiKey":"sk-local"}
        ]);
        let remote = json!([
            {"id":"provider-a","name":"Remote","baseUrl":"https://remote.example/v1","apiKey":"sk-remote"}
        ]);

        let merged = merge_provider_values(&local, &remote, SyncStrategy::SmartMerge)
            .expect("merge providers");
        let provider = merged.as_array().expect("array").first().expect("provider");

        assert_eq!(
            provider.get("apiKey").and_then(serde_json::Value::as_str),
            Some("sk-local")
        );
        assert_eq!(
            provider.get("name").and_then(serde_json::Value::as_str),
            Some("Remote")
        );
        assert_eq!(
            provider.get("baseUrl").and_then(serde_json::Value::as_str),
            Some("https://remote.example/v1")
        );
    }

    #[test]
    fn remote_overwrite_creates_backup_before_writing_models_and_providers() {
        let local = temp_root("overwrite-local");
        write(
            &local.join("models.json"),
            r#"[{"id":"local","apiKey":"sk-local"}]"#,
        );
        write(
            &local.join("model-providers.json"),
            r#"[{"id":"provider","apiKey":"sk-local"}]"#,
        );

        let remote = temp_root("overwrite-remote");
        write(
            &remote.join("models.json"),
            r#"[{"id":"remote","apiKey":"sk-remote"}]"#,
        );
        write(
            &remote.join("model-providers.json"),
            r#"[{"id":"provider","apiKey":"sk-remote"}]"#,
        );
        let package = remote.join("remote.zip");
        build_sync_package(&remote, &package).expect("build remote package");

        let result = apply_sync_package(&local, &package, SyncStrategy::RemoteOverwriteLocal)
            .expect("apply package");

        assert!(result.backup_dir.is_some());
        let backup_dir = result.backup_dir.expect("backup dir");
        assert!(backup_dir.join("models.json").exists());
        assert!(backup_dir.join("model-providers.json").exists());
        assert!(fs::read_to_string(local.join("models.json"))
            .expect("models")
            .contains("remote"));
        fs::remove_dir_all(local).ok();
        fs::remove_dir_all(remote).ok();
    }

    #[test]
    fn apply_rewrites_remote_default_workspace_paths_to_local_default() {
        let remote = temp_root("remote-workspace-path");
        let local = temp_root("local-workspace-path");
        let remote_default = r"D:\OneDrive\WorkBuddy\WorkSpace";
        let local_default = r"E:\OneDrive\WorkBuddy\WorkSpace";
        let remote_session_cwd = format!(r"{}\ProjectA", remote_default);
        let local_session_cwd = format!(r"{}\ProjectA", local_default);
        let remote_workspace_path = format!(r"{}\ProjectB", remote_default);
        let local_workspace_path = format!(r"{}\ProjectB", local_default);

        write_app_config(&remote, remote_default);
        create_sync_database(&remote, Some(&remote_session_cwd), None);
        let package = remote.join("remote.zip");
        build_sync_package(&remote, &package).expect("build remote package");

        write_app_config(&local, local_default);
        create_sync_database(&local, None, Some(&remote_workspace_path));

        apply_sync_package(&local, &package, SyncStrategy::SmartMerge).expect("apply package");

        let connection = Connection::open(local.join("workbuddy.db")).expect("open local db");
        let actual_session_cwd: String = connection
            .query_row(
                "SELECT cwd FROM sessions WHERE id = 'session-1'",
                [],
                |row| row.get(0),
            )
            .expect("read session cwd");
        let actual_workspace_path: String = connection
            .query_row(
                "SELECT path FROM workspaces WHERE id = 'workspace-1'",
                [],
                |row| row.get(0),
            )
            .expect("read workspace path");

        assert_eq!(actual_session_cwd, local_session_cwd);
        assert_eq!(actual_workspace_path, local_workspace_path);

        fs::remove_dir_all(local).ok();
        fs::remove_dir_all(remote).ok();
    }

    #[test]
    fn apply_rejects_package_when_manifest_hash_does_not_match() {
        let local = temp_root("tampered-local");
        let package = local.join("tampered.zip");
        let file = fs::File::create(&package).expect("create tampered zip");
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file(MODELS_ARCHIVE_PATH, options)
            .expect("models entry");
        zip.write_all(br#"[{"id":"remote"}]"#)
            .expect("models bytes");
        let manifest = SyncPackageManifest {
            schema_version: 1,
            package_format: "workbuddy-webdav-zip-sync".to_string(),
            created_at: "2026-07-12T00:00:00Z".to_string(),
            files: vec![SyncPackageFile {
                path: MODELS_ARCHIVE_PATH.to_string(),
                size: 17,
                sha256: "not-the-real-hash".to_string(),
            }],
        };
        zip.start_file(MANIFEST_PATH, options)
            .expect("manifest entry");
        zip.write_all(&serde_json::to_vec(&manifest).expect("manifest json"))
            .expect("manifest bytes");
        zip.finish().expect("finish tampered zip");

        let error = apply_sync_package(&local, &package, SyncStrategy::RemoteOverwriteLocal)
            .expect_err("tampered package must be rejected");

        assert!(error.contains("校验") || error.contains("hash"));
        fs::remove_dir_all(local).ok();
    }
}
