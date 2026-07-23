use chrono::Utc;
use rusqlite::{
    backup::Backup, params_from_iter, types::Value as SqlValue, Connection, OptionalExtension,
    Transaction,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    time::UNIX_EPOCH,
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
const SESSION_BLOBS_PREFIX: &str = "workbuddy-sync/sessions/blobs/";
const ARTIFACT_INDEX_PREFIX: &str = "workbuddy-sync/sessions/artifact-index/";
const PROFILE_PREFIX: &str = "workbuddy-sync/profile/";
const PROFILE_MEMORY_PREFIX: &str = "workbuddy-sync/profile/memory/";
const PORTABLE_APP_CONFIG_PATH: &str = "workbuddy-sync/preferences/app-config.json";
const PROFILE_FILES: [&str; 4] = ["MEMORY.md", "USER.md", "SOUL.md", "IDENTITY.md"];

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

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PortableAppConfigExport {
    schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    modified_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    disable_agent_teams: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    personalization: Option<Value>,
}

#[derive(Debug)]
struct SessionAssetReferences {
    content_hashes: HashSet<String>,
    session_ids: HashSet<String>,
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
    let mut conflicts = apply_session_files(workbuddy_dir, &entries, strategy)?;
    apply_profile_files(workbuddy_dir, &entries)?;
    apply_session_blobs(workbuddy_dir, &entries, &mut conflicts)?;
    apply_artifact_indexes(workbuddy_dir, &entries)?;
    apply_portable_app_config(workbuddy_dir, &entries, strategy)?;
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

    let app_config = workbuddy_dir.join("app").join("app-config.json");
    if app_config.exists() {
        let target = backup_dir.join("app").join("app-config.json");
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|err| format!("创建应用配置备份目录失败：{err}"))?;
        }
        fs::copy(&app_config, &target)
            .map_err(|err| format!("备份 app-config.json 失败：{err}"))?;
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
    collect_profile_entries(workbuddy_dir, &mut entries)?;
    collect_session_asset_entries(workbuddy_dir, &mut entries)?;
    collect_portable_app_config(workbuddy_dir, &mut entries)?;
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

fn collect_profile_entries(
    workbuddy_dir: &Path,
    entries: &mut Vec<PackageEntry>,
) -> Result<(), String> {
    for name in PROFILE_FILES {
        push_optional_file(
            entries,
            workbuddy_dir.join(name),
            &format!("{PROFILE_PREFIX}{name}"),
        )?;
    }

    let memory_dir = workbuddy_dir.join("memory");
    if !memory_dir.exists() {
        return Ok(());
    }
    for entry in WalkDir::new(&memory_dir)
        .min_depth(1)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        let relative = path
            .strip_prefix(&memory_dir)
            .map_err(|err| format!("计算记忆文件相对路径失败：{err}"))?;
        entries.push(PackageEntry {
            path: format!("{PROFILE_MEMORY_PREFIX}{}", path_to_archive_path(relative)?),
            bytes: fs::read(path).map_err(|err| format!("读取记忆文件失败：{err}"))?,
        });
    }
    Ok(())
}

fn collect_session_asset_entries(
    workbuddy_dir: &Path,
    entries: &mut Vec<PackageEntry>,
) -> Result<(), String> {
    let references = collect_session_asset_references(workbuddy_dir)?;
    let blobs_dir = workbuddy_dir.join("blobs");
    if blobs_dir.exists() {
        for entry in WalkDir::new(&blobs_dir)
            .min_depth(1)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
        {
            let path = entry.path();
            let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            if !references
                .content_hashes
                .contains(&stem.to_ascii_lowercase())
            {
                continue;
            }
            let relative = path
                .strip_prefix(&blobs_dir)
                .map_err(|err| format!("计算 Blob 相对路径失败：{err}"))?;
            entries.push(PackageEntry {
                path: format!("{SESSION_BLOBS_PREFIX}{}", path_to_archive_path(relative)?),
                bytes: fs::read(path).map_err(|err| format!("读取会话 Blob 失败：{err}"))?,
            });
        }
    }

    let artifact_dir = workbuddy_dir.join("artifact-index");
    if artifact_dir.exists() {
        for entry in WalkDir::new(&artifact_dir)
            .min_depth(1)
            .max_depth(1)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
        {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let Some(session_id) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            if !references.session_ids.contains(session_id) {
                continue;
            }
            entries.push(PackageEntry {
                path: format!("{ARTIFACT_INDEX_PREFIX}{session_id}.json"),
                bytes: fs::read(path).map_err(|err| format!("读取 artifact-index 失败：{err}"))?,
            });
        }
    }
    Ok(())
}

fn collect_session_asset_references(
    workbuddy_dir: &Path,
) -> Result<SessionAssetReferences, String> {
    let mut references = SessionAssetReferences {
        content_hashes: HashSet::new(),
        session_ids: HashSet::new(),
    };
    let projects_dir = workbuddy_dir.join("projects");
    if !projects_dir.exists() {
        return Ok(references);
    }

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
        let bytes = fs::read(path).map_err(|err| format!("读取会话附件引用失败：{err}"))?;
        collect_hex_hashes(&bytes, &mut references.content_hashes);
        for line in bytes.split(|byte| *byte == b'\n') {
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let Ok(value) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            collect_named_strings(&value, "sessionId", &mut references.session_ids);
        }
    }
    Ok(references)
}

fn collect_hex_hashes(bytes: &[u8], output: &mut HashSet<String>) {
    let mut start = 0;
    while start < bytes.len() {
        while start < bytes.len() && !bytes[start].is_ascii_hexdigit() {
            start += 1;
        }
        let mut end = start;
        while end < bytes.len() && bytes[end].is_ascii_hexdigit() {
            end += 1;
        }
        if end.saturating_sub(start) == 64 {
            output.insert(String::from_utf8_lossy(&bytes[start..end]).to_ascii_lowercase());
        }
        start = end.max(start + 1);
    }
}

fn collect_named_strings(value: &Value, key: &str, output: &mut HashSet<String>) {
    match value {
        Value::Object(object) => {
            for (name, value) in object {
                if name == key {
                    if let Some(text) = value.as_str().filter(|text| !text.trim().is_empty()) {
                        output.insert(text.to_string());
                    }
                }
                collect_named_strings(value, key, output);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_named_strings(value, key, output);
            }
        }
        _ => {}
    }
}

fn collect_portable_app_config(
    workbuddy_dir: &Path,
    entries: &mut Vec<PackageEntry>,
) -> Result<(), String> {
    let path = workbuddy_dir.join("app").join("app-config.json");
    if !path.exists() {
        return Ok(());
    }
    let bytes = fs::read(&path).map_err(|err| format!("读取 WorkBuddy 应用配置失败：{err}"))?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|err| format!("解析 WorkBuddy 应用配置失败：{err}"))?;
    let export = PortableAppConfigExport {
        schema_version: 1,
        modified_at_ms: file_modified_at_ms(&path),
        disable_agent_teams: value.get("disableAgentTeams").and_then(Value::as_bool),
        personalization: value.get("personalization").cloned(),
    };
    entries.push(PackageEntry {
        path: PORTABLE_APP_CONFIG_PATH.to_string(),
        bytes: serde_json::to_vec_pretty(&export)
            .map_err(|err| format!("序列化便携应用配置失败：{err}"))?,
    });
    Ok(())
}

fn file_modified_at_ms(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis()
        .try_into()
        .ok()
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
    )?;
    apply_json_array_file(
        &workbuddy_dir.join("model-providers.json"),
        entries.get(PROVIDERS_ARCHIVE_PATH),
        strategy,
    )
}

fn apply_json_array_file(
    target: &Path,
    remote_bytes: Option<&Vec<u8>>,
    strategy: SyncStrategy,
) -> Result<(), String> {
    if strategy != SyncStrategy::RemoteOverwriteLocal {
        return Ok(());
    }
    let Some(remote_bytes) = remote_bytes else {
        return Ok(());
    };
    let remote_value: Value = serde_json::from_slice(remote_bytes)
        .map_err(|err| format!("解析远端 JSON 配置失败：{err}"))?;
    write_json_value(target, &remote_value)
}

fn apply_profile_files(
    workbuddy_dir: &Path,
    entries: &HashMap<String, Vec<u8>>,
) -> Result<(), String> {
    let mut archive_paths = entries
        .keys()
        .filter(|path| path.starts_with(PROFILE_PREFIX))
        .cloned()
        .collect::<Vec<_>>();
    archive_paths.sort();

    for archive_path in archive_paths {
        let relative = archive_path
            .strip_prefix(PROFILE_PREFIX)
            .ok_or_else(|| "用户记忆同步路径无效".to_string())?;
        let target = if let Some(memory_relative) = relative.strip_prefix("memory/") {
            workbuddy_dir.join("memory").join(memory_relative)
        } else if PROFILE_FILES.contains(&relative) {
            workbuddy_dir.join(relative)
        } else {
            continue;
        };
        let remote_bytes = entries
            .get(&archive_path)
            .ok_or_else(|| "用户记忆同步条目缺失".to_string())?;
        let remote = String::from_utf8(remote_bytes.clone())
            .map_err(|err| format!("远端用户记忆不是 UTF-8：{err}"))?;
        let next = if target.exists() {
            let local = fs::read_to_string(&target)
                .map_err(|err| format!("读取本地用户记忆失败：{err}"))?;
            merge_markdown(&local, &remote)
        } else {
            remote
        };
        write_text_if_changed(&target, &next)?;
    }
    Ok(())
}

#[derive(Debug)]
struct MarkdownSection {
    heading: Option<String>,
    lines: Vec<String>,
}

fn merge_markdown(local: &str, remote: &str) -> String {
    let local = normalize_newlines(local);
    let remote = normalize_newlines(remote);
    if local.trim().is_empty() {
        return ensure_trailing_newline(&remote);
    }
    if remote.trim().is_empty() || local == remote || local.contains(remote.trim()) {
        return ensure_trailing_newline(&local);
    }
    if remote.contains(local.trim()) {
        return ensure_trailing_newline(&remote);
    }

    let mut merged = markdown_sections(&local);
    for remote_section in markdown_sections(&remote) {
        let matching = merged
            .iter_mut()
            .find(|section| section.heading == remote_section.heading);
        if let Some(local_section) = matching {
            let existing = local_section
                .lines
                .iter()
                .map(|line| line.trim().to_string())
                .filter(|line| !line.is_empty())
                .collect::<HashSet<_>>();
            let additions = remote_section
                .lines
                .into_iter()
                .filter(|line| !line.trim().is_empty() && !existing.contains(line.trim()))
                .collect::<Vec<_>>();
            if !additions.is_empty() {
                if local_section
                    .lines
                    .last()
                    .is_some_and(|line| !line.trim().is_empty())
                {
                    local_section.lines.push(String::new());
                }
                local_section.lines.extend(additions);
            }
        } else {
            merged.push(remote_section);
        }
    }
    let mut output = String::new();
    for (index, section) in merged.into_iter().enumerate() {
        if index > 0 && !output.ends_with("\n\n") {
            if !output.ends_with('\n') {
                output.push('\n');
            }
            output.push('\n');
        }
        if let Some(heading) = section.heading {
            output.push_str(&heading);
            output.push('\n');
        }
        output.push_str(&section.lines.join("\n"));
    }
    ensure_trailing_newline(output.trim_end())
}

fn markdown_sections(content: &str) -> Vec<MarkdownSection> {
    let mut sections = vec![MarkdownSection {
        heading: None,
        lines: Vec::new(),
    }];
    for line in content.lines() {
        if is_markdown_heading(line) {
            sections.push(MarkdownSection {
                heading: Some(line.trim_end().to_string()),
                lines: Vec::new(),
            });
        } else if let Some(section) = sections.last_mut() {
            section.lines.push(line.to_string());
        }
    }
    sections
        .into_iter()
        .filter(|section| {
            section.heading.is_some() || section.lines.iter().any(|line| !line.trim().is_empty())
        })
        .collect()
}

fn is_markdown_heading(line: &str) -> bool {
    let trimmed = line.trim_start();
    let hashes = trimmed.bytes().take_while(|byte| *byte == b'#').count();
    (1..=6).contains(&hashes) && trimmed.as_bytes().get(hashes) == Some(&b' ')
}

fn normalize_newlines(content: &str) -> String {
    content.replace("\r\n", "\n").replace('\r', "\n")
}

fn ensure_trailing_newline(content: &str) -> String {
    let mut output = content.to_string();
    if !output.ends_with('\n') {
        output.push('\n');
    }
    output
}

fn write_text_if_changed(path: &Path, content: &str) -> Result<(), String> {
    if path.exists()
        && fs::read_to_string(path).map_err(|err| format!("读取文本文件失败：{err}"))? == content
    {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("创建文本文件目录失败：{err}"))?;
    }
    fs::write(path, content).map_err(|err| format!("写入文本文件失败：{err}"))
}

fn apply_session_blobs(
    workbuddy_dir: &Path,
    entries: &HashMap<String, Vec<u8>>,
    conflicts: &mut Vec<String>,
) -> Result<(), String> {
    for (archive_path, remote_bytes) in entries {
        let Some(relative) = archive_path.strip_prefix(SESSION_BLOBS_PREFIX) else {
            continue;
        };
        let target = workbuddy_dir.join("blobs").join(relative);
        if !target.exists() {
            write_binary_file(&target, remote_bytes)?;
            continue;
        }
        let local_bytes = fs::read(&target).map_err(|err| format!("读取本地 Blob 失败：{err}"))?;
        if local_bytes == *remote_bytes {
            continue;
        }
        let conflict = binary_conflict_path(&target);
        write_binary_file(&conflict, remote_bytes)?;
        conflicts.push(conflict.to_string_lossy().to_string());
    }
    Ok(())
}

fn write_binary_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("创建二进制文件目录失败：{err}"))?;
    }
    fs::write(path, bytes).map_err(|err| format!("写入二进制同步文件失败：{err}"))
}

fn binary_conflict_path(target: &Path) -> PathBuf {
    let timestamp = Utc::now().format("%Y%m%dT%H%M%SZ");
    let name = target
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("blob");
    target.with_file_name(format!("{name}.conflict.{timestamp}"))
}

fn apply_artifact_indexes(
    workbuddy_dir: &Path,
    entries: &HashMap<String, Vec<u8>>,
) -> Result<(), String> {
    for (archive_path, remote_bytes) in entries {
        let Some(relative) = archive_path.strip_prefix(ARTIFACT_INDEX_PREFIX) else {
            continue;
        };
        let target = workbuddy_dir.join("artifact-index").join(relative);
        let remote: Value = serde_json::from_slice(remote_bytes)
            .map_err(|err| format!("解析远端 artifact-index 失败：{err}"))?;
        let next = if target.exists() {
            let local: Value = serde_json::from_slice(
                &fs::read(&target).map_err(|err| format!("读取本地 artifact-index 失败：{err}"))?,
            )
            .map_err(|err| format!("解析本地 artifact-index 失败：{err}"))?;
            merge_artifact_index(&local, &remote)?
        } else {
            remote
        };
        write_json_value(&target, &next)?;
    }
    Ok(())
}

fn merge_artifact_index(local: &Value, remote: &Value) -> Result<Value, String> {
    let local_object = local
        .as_object()
        .ok_or_else(|| "本地 artifact-index 不是对象".to_string())?;
    let remote_object = remote
        .as_object()
        .ok_or_else(|| "远端 artifact-index 不是对象".to_string())?;
    let mut output = local_object.clone();
    let mut artifacts = Vec::new();
    let mut positions = HashMap::<String, usize>::new();

    for artifact in local_object
        .get("artifacts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        positions.insert(artifact_identity(artifact), artifacts.len());
        artifacts.push(artifact.clone());
    }
    for artifact in remote_object
        .get("artifacts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let identity = artifact_identity(artifact);
        if let Some(position) = positions.get(&identity).copied() {
            if artifact_timestamp(artifact) > artifact_timestamp(&artifacts[position]) {
                artifacts[position] = artifact.clone();
            }
        } else {
            positions.insert(identity, artifacts.len());
            artifacts.push(artifact.clone());
        }
    }

    output.insert("artifacts".to_string(), Value::Array(artifacts));
    output.insert(
        "version".to_string(),
        Value::from(max_numeric_field(local_object, remote_object, "version")),
    );
    output.insert(
        "lastUpdated".to_string(),
        Value::from(max_numeric_field(
            local_object,
            remote_object,
            "lastUpdated",
        )),
    );
    Ok(Value::Object(output))
}

fn artifact_identity(value: &Value) -> String {
    for field in ["uri", "id"] {
        if let Some(identity) = value.get(field).and_then(Value::as_str) {
            return format!("{field}:{identity}");
        }
    }
    let artifact_type = value.get("type").and_then(Value::as_str).unwrap_or("");
    let name = value.get("name").and_then(Value::as_str).unwrap_or("");
    let created_at = value.get("createdAt").and_then(Value::as_i64).unwrap_or(0);
    format!("fallback:{artifact_type}:{name}:{created_at}")
}

fn artifact_timestamp(value: &Value) -> i64 {
    value
        .get("updatedAt")
        .and_then(Value::as_i64)
        .or_else(|| value.get("createdAt").and_then(Value::as_i64))
        .unwrap_or(0)
}

fn max_numeric_field(
    local: &serde_json::Map<String, Value>,
    remote: &serde_json::Map<String, Value>,
    field: &str,
) -> i64 {
    local
        .get(field)
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .max(remote.get(field).and_then(Value::as_i64).unwrap_or(0))
}

fn apply_portable_app_config(
    workbuddy_dir: &Path,
    entries: &HashMap<String, Vec<u8>>,
    strategy: SyncStrategy,
) -> Result<(), String> {
    if strategy == SyncStrategy::LocalOverwriteRemote {
        return Ok(());
    }
    let Some(remote_bytes) = entries.get(PORTABLE_APP_CONFIG_PATH) else {
        return Ok(());
    };
    let remote: PortableAppConfigExport = serde_json::from_slice(remote_bytes)
        .map_err(|err| format!("解析远端便携应用配置失败：{err}"))?;
    let target = workbuddy_dir.join("app").join("app-config.json");
    let local_modified_at = file_modified_at_ms(&target);
    let mut local = if target.exists() {
        serde_json::from_slice::<Value>(
            &fs::read(&target).map_err(|err| format!("读取本地应用配置失败：{err}"))?,
        )
        .map_err(|err| format!("解析本地应用配置失败：{err}"))?
    } else {
        Value::Object(serde_json::Map::new())
    };
    if strategy == SyncStrategy::SmartMerge
        && has_non_default_portable_app_config(&local)
        && local_modified_at.is_some()
        && remote.modified_at_ms.is_some()
        && local_modified_at >= remote.modified_at_ms
    {
        return Ok(());
    }
    let object = local
        .as_object_mut()
        .ok_or_else(|| "本地应用配置不是对象".to_string())?;
    if let Some(value) = remote.disable_agent_teams {
        object.insert("disableAgentTeams".to_string(), Value::Bool(value));
    }
    if let Some(value) = remote.personalization {
        object.insert("personalization".to_string(), value);
    }
    write_json_value(&target, &local)
}

fn has_non_default_portable_app_config(value: &Value) -> bool {
    if value
        .get("disableAgentTeams")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return true;
    }
    value
        .get("personalization")
        .and_then(Value::as_object)
        .is_some_and(|personalization| {
            personalization.values().any(|value| match value {
                Value::String(text) => !text.trim().is_empty(),
                Value::Null => false,
                _ => true,
            })
        })
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
    let local = trim_trailing_path_separators(local?).to_string();
    if local.is_empty() {
        return None;
    }
    // 即使远端和本机默认目录相同，也要保留 rewrite：远端快照中可能
    // 混有 D:\WorkSpace 等默认目录之外的绝对路径，需要将其归入本机
    // defaultWorkspacePath。旧逻辑在 remote == local 时会整体跳过。
    let remote = remote
        .map(trim_trailing_path_separators)
        .filter(|value| !value.is_empty())
        .unwrap_or(&local)
        .to_string();
    Some(WorkspacePathRewrite { remote, local })
}

fn rewrite_workspace_paths(
    transaction: &Transaction<'_>,
    rewrite: &WorkspacePathRewrite,
) -> Result<(), String> {
    rewrite_table_path_column(transaction, "sessions", "cwd", rewrite, false)?;
    rewrite_table_path_column(transaction, "workspaces", "path", rewrite, true)
}

fn rewrite_table_path_column(
    transaction: &Transaction<'_>,
    table: &str,
    column: &str,
    rewrite: &WorkspacePathRewrite,
    keep_existing_target: bool,
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
    let target_exists_sql =
        format!("SELECT 1 FROM {table_identifier} WHERE {column_identifier} = ?1 LIMIT 1");
    let delete_source_sql =
        format!("DELETE FROM {table_identifier} WHERE {column_identifier} = ?1");
    for (old_path, new_path) in replacements {
        // workspaces.path is unique in WorkBuddy. A project may already have a
        // row using the repaired local path, so updating the stale remote-path
        // row directly would violate that constraint. Keep the existing local
        // row and remove only the duplicate stale row.
        let target_exists = keep_existing_target
            && transaction
                .query_row(&target_exists_sql, [&new_path], |_| Ok(()))
                .optional()
                .map_err(|err| format!("检查修复后的 {table}.{column} 路径是否存在失败：{err}"))?
                .is_some();
        if target_exists {
            transaction
                .execute(&delete_source_sql, [&old_path])
                .map_err(|err| format!("合并重复 {table}.{column} 路径失败：{err}"))?;
            continue;
        }
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
    let local = trim_trailing_path_separators(&rewrite.local);

    // 本来就在本机默认目录下，不重复改写。
    if workspace_prefix_len(value, local).is_some() {
        return None;
    }

    // 首选精确替换远端默认目录前缀。
    if canonical_workspace_path(&rewrite.remote) != canonical_workspace_path(local) {
        if let Some(prefix_len) = workspace_prefix_len(value, &rewrite.remote) {
            return Some(join_workspace_path(local, &value[prefix_len..]));
        }
    }

    // 远端会话可能使用默认目录之外、但位于同一远端磁盘的工程目录。
    // 此时仅映射远端默认目录盘符到本机盘符，保留完整目录结构。
    rewrite_windows_drive(value, &rewrite.remote, local)
}

fn rewrite_windows_drive(value: &str, remote_root: &str, local_root: &str) -> Option<String> {
    let value_drive = windows_drive_letter(value)?;
    let remote_drive = windows_drive_letter(remote_root)?;
    let local_drive = windows_drive_letter(local_root)?;
    if value_drive != remote_drive || remote_drive == local_drive {
        return None;
    }

    let mut characters = value.trim().chars();
    characters.next()?;
    Some(format!("{local_drive}{}", characters.as_str()))
}

fn join_workspace_path(root: &str, suffix: &str) -> String {
    let root = trim_trailing_path_separators(root);
    let suffix = suffix.trim_start_matches(is_path_separator);
    if suffix.is_empty() {
        return root.to_string();
    }
    let separator = if root.contains('\\') || is_windows_drive_path(root) {
        '\\'
    } else {
        '/'
    };
    format!("{root}{separator}{suffix}")
}

fn windows_drive_letter(value: &str) -> Option<char> {
    let mut characters = value.trim().chars();
    let drive = characters.next()?;
    if !drive.is_ascii_alphabetic() || characters.next()? != ':' {
        return None;
    }
    if !is_path_separator(characters.next()?) {
        return None;
    }
    Some(drive.to_ascii_uppercase())
}

fn is_windows_drive_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
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
                    path TEXT UNIQUE
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
        let blob_id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        write(
            &root.join("projects/cwd-key/session-1.jsonl"),
            &format!(
                "{{\"type\":\"message\",\"sessionId\":\"session-1\",\"blob_id\":\"{blob_id}\"}}\n"
            ),
        );
        write(&root.join(format!("blobs/aa/{blob_id}.png")), "blob");
        write(
            &root.join(
                "blobs/bb/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.png",
            ),
            "unreferenced",
        );
        write(
            &root.join("artifact-index/session-1.json"),
            r#"{"version":1,"lastUpdated":10,"artifacts":[]}"#,
        );
        write(&root.join("MEMORY.md"), "# Memory\n");
        write(&root.join("memory/profile.md"), "# Profile\n");
        write(&root.join("memory/profile.md.bak"), "backup");
        write(&root.join("sessions/12345.json"), "{\"pid\":12345}");
        write(&root.join("app/session/Cache/blob"), "cache");
        write(&root.join("app/sessions.json"), "[]");
        write(
            &root.join("app/app-config.json"),
            r#"{"defaultWorkspacePath":"D:\\Local","disableAgentTeams":true,"personalization":{"toneStyle":"concise","customPrompt":"hello"}}"#,
        );
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
        assert!(paths.contains(&"workbuddy-sync/profile/MEMORY.md"));
        assert!(paths.contains(&"workbuddy-sync/profile/memory/profile.md"));
        assert!(paths.contains(&"workbuddy-sync/sessions/artifact-index/session-1.json"));
        assert!(paths.contains(&format!("workbuddy-sync/sessions/blobs/aa/{blob_id}.png").as_str()));
        assert!(paths.contains(&PORTABLE_APP_CONFIG_PATH));
        assert!(!paths.iter().any(|path| path.ends_with("profile.md.bak")));
        assert!(!paths.iter().any(|path| path.contains("bbbbbbbbbbbbbbbb")));
        assert!(!paths
            .iter()
            .any(|path| path.contains("sessions/12345.json")));
        assert!(!paths.iter().any(|path| path.contains("app/session")));
        assert!(!paths.iter().any(|path| path.ends_with("app/sessions.json")));

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn markdown_merge_keeps_unique_content_in_matching_sections() {
        let local = "# Profile\n\n## Preferences\n\n- Local choice\n";
        let remote =
            "# Profile\n\n## Preferences\n\n- Remote choice\n\n## Environment\n\n- Windows\n";

        let merged = merge_markdown(local, remote);

        assert_eq!(merged.matches("# Profile").count(), 1);
        assert_eq!(merged.matches("## Preferences").count(), 1);
        assert!(merged.contains("- Local choice"));
        assert!(merged.contains("- Remote choice"));
        assert!(merged.contains("## Environment\n\n- Windows"));
    }

    #[test]
    fn artifact_index_merge_unions_artifacts_and_keeps_newer_duplicate() {
        let local = json!({
            "version": 1,
            "lastUpdated": 20,
            "artifacts": [
                {"uri":"agent://shared","title":"Local","updatedAt":10},
                {"uri":"agent://local","title":"Local only","updatedAt":20}
            ]
        });
        let remote = json!({
            "version": 2,
            "lastUpdated": 30,
            "artifacts": [
                {"uri":"agent://shared","title":"Remote","updatedAt":30},
                {"uri":"agent://remote","title":"Remote only","updatedAt":25}
            ]
        });

        let merged = merge_artifact_index(&local, &remote).expect("merge artifact index");
        let artifacts = merged["artifacts"].as_array().expect("artifacts");

        assert_eq!(merged["version"], 2);
        assert_eq!(merged["lastUpdated"], 30);
        assert_eq!(artifacts.len(), 3);
        assert_eq!(
            artifacts
                .iter()
                .find(|artifact| artifact["uri"] == "agent://shared")
                .expect("shared")["title"],
            "Remote"
        );
    }

    #[test]
    fn apply_merges_profile_blobs_artifacts_and_portable_app_config() {
        let blob_id = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
        let remote = temp_root("remote-portable-data");
        write(
            &remote.join("projects/key/session-1.jsonl"),
            &format!(
                "{{\"type\":\"message\",\"sessionId\":\"session-1\",\"blob_id\":\"{blob_id}\"}}\n"
            ),
        );
        write(
            &remote.join(format!("blobs/cc/{blob_id}.png")),
            "blob-bytes",
        );
        write(
            &remote.join("MEMORY.md"),
            "# Memory\n\n## Facts\n\n- Remote fact\n",
        );
        write(
            &remote.join("memory/profile.md"),
            "# Profile\n\n- Remote profile\n",
        );
        write(
            &remote.join("artifact-index/session-1.json"),
            r#"{"version":1,"lastUpdated":30,"artifacts":[{"uri":"agent://remote","updatedAt":30}]}"#,
        );
        write(
            &remote.join("app/app-config.json"),
            r#"{"defaultWorkspacePath":"D:\\Remote","disableAgentTeams":true,"personalization":{"toneStyle":"concise","customPrompt":"remote prompt"}}"#,
        );
        let package = remote.join("remote.zip");
        build_sync_package(&remote, &package).expect("build remote package");

        let local = temp_root("local-portable-data");
        write(
            &local.join("MEMORY.md"),
            "# Memory\n\n## Facts\n\n- Local fact\n",
        );
        write(
            &local.join("artifact-index/session-1.json"),
            r#"{"version":1,"lastUpdated":20,"artifacts":[{"uri":"agent://local","updatedAt":20}]}"#,
        );
        write(
            &local.join("app/app-config.json"),
            r#"{"defaultWorkspacePath":"E:\\Local","disableAgentTeams":false,"personalization":{"toneStyle":"","customPrompt":""}}"#,
        );

        apply_sync_package(&local, &package, SyncStrategy::RemoteOverwriteLocal)
            .expect("apply remote package");

        let memory = fs::read_to_string(local.join("MEMORY.md")).expect("memory");
        assert!(memory.contains("- Local fact"));
        assert!(memory.contains("- Remote fact"));
        assert_eq!(
            fs::read(local.join(format!("blobs/cc/{blob_id}.png"))).expect("blob"),
            b"blob-bytes"
        );
        let artifacts: Value = serde_json::from_slice(
            &fs::read(local.join("artifact-index/session-1.json")).expect("artifact index"),
        )
        .expect("parse artifact index");
        assert_eq!(
            artifacts["artifacts"].as_array().expect("artifacts").len(),
            2
        );
        let app_config: Value = serde_json::from_slice(
            &fs::read(local.join("app/app-config.json")).expect("app config"),
        )
        .expect("parse app config");
        assert_eq!(app_config["defaultWorkspacePath"], r"E:\Local");
        assert_eq!(app_config["disableAgentTeams"], true);
        assert_eq!(
            app_config["personalization"]["customPrompt"],
            "remote prompt"
        );
        assert!(local.join("memory/profile.md").exists());

        fs::remove_dir_all(local).ok();
        fs::remove_dir_all(remote).ok();
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
    fn smart_merge_leaves_local_models_and_providers_unchanged() {
        let local = temp_root("smart-merge-local-config");
        let local_models = r#"[{"id":"local-model","apiKey":"sk-local-model"}]"#;
        let local_providers = r#"[{"id":"local-provider","apiKey":"sk-local-provider"}]"#;
        write(&local.join("models.json"), local_models);
        write(&local.join("model-providers.json"), local_providers);

        let remote = temp_root("smart-merge-remote-config");
        write(
            &remote.join("models.json"),
            r#"[{"id":"remote-model","apiKey":"sk-remote-model"}]"#,
        );
        write(
            &remote.join("model-providers.json"),
            r#"[{"id":"remote-provider","apiKey":"sk-remote-provider"}]"#,
        );
        let package = remote.join("remote.zip");
        build_sync_package(&remote, &package).expect("build remote package");

        apply_sync_package(&local, &package, SyncStrategy::SmartMerge)
            .expect("apply smart merge package");

        assert_eq!(
            fs::read_to_string(local.join("models.json")).expect("models"),
            local_models
        );
        assert_eq!(
            fs::read_to_string(local.join("model-providers.json")).expect("providers"),
            local_providers
        );
        fs::remove_dir_all(local).ok();
        fs::remove_dir_all(remote).ok();
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
    fn apply_merges_workspace_when_repaired_path_already_exists() {
        let remote = temp_root("remote-duplicate-workspace-path");
        let local = temp_root("local-duplicate-workspace-path");
        let remote_default = r"D:\OneDrive\WorkBuddy\WorkSpace";
        let local_default = r"E:\OneDrive\WorkBuddy\WorkSpace";
        let remote_project = format!(r"{}\ProjectA", remote_default);
        let local_project = format!(r"{}\ProjectA", local_default);

        write_app_config(&remote, remote_default);
        create_sync_database(&remote, Some(&remote_project), None);
        let package = remote.join("remote.zip");
        build_sync_package(&remote, &package).expect("build remote package");

        write_app_config(&local, local_default);
        create_sync_database(&local, None, Some(&remote_project));
        let connection = Connection::open(local.join("workbuddy.db")).expect("open local db");
        connection
            .execute(
                "INSERT INTO workspaces (id, path) VALUES ('workspace-local', ?1)",
                [&local_project],
            )
            .expect("insert existing local workspace");
        drop(connection);

        apply_sync_package(&local, &package, SyncStrategy::SmartMerge).expect("apply package");

        let connection = Connection::open(local.join("workbuddy.db")).expect("open local db");
        let workspaces = connection
            .prepare("SELECT id, path FROM workspaces ORDER BY id")
            .expect("prepare workspace query")
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .expect("query workspaces")
            .collect::<Result<Vec<_>, _>>()
            .expect("read workspaces");
        let session_cwd: String = connection
            .query_row(
                "SELECT cwd FROM sessions WHERE id = 'session-1'",
                [],
                |row| row.get(0),
            )
            .expect("read session cwd");

        assert_eq!(
            workspaces,
            vec![("workspace-local".to_string(), local_project.clone())]
        );
        assert_eq!(session_cwd, local_project);

        fs::remove_dir_all(local).ok();
        fs::remove_dir_all(remote).ok();
    }

    #[test]
    fn apply_keeps_foreign_absolute_paths_when_defaults_are_equal() {
        let remote = temp_root("remote-foreign-workspace-path");
        let local = temp_root("local-foreign-workspace-path");
        let default = r"E:\OneDrive\WorkBuddy";
        let foreign_session = r"D:\WorkSpace\fanruan\fine-passport\fine-passport-springboot";

        write_app_config(&remote, default);
        create_sync_database(&remote, Some(foreign_session), None);
        let package = remote.join("remote.zip");
        build_sync_package(&remote, &package).expect("build remote package");

        write_app_config(&local, default);
        create_sync_database(&local, None, None);
        apply_sync_package(&local, &package, SyncStrategy::SmartMerge).expect("apply package");

        let connection = Connection::open(local.join("workbuddy.db")).expect("open local db");
        let actual: String = connection
            .query_row(
                "SELECT cwd FROM sessions WHERE id = 'session-1'",
                [],
                |row| row.get(0),
            )
            .expect("read session cwd");
        assert_eq!(actual, foreign_session);

        fs::remove_dir_all(local).ok();
        fs::remove_dir_all(remote).ok();
    }

    #[test]
    fn workspace_rewrite_keeps_local_and_relative_paths_unchanged() {
        let rewrite = workspace_path_rewrite(
            Some(r"E:\OneDrive\WorkBuddy"),
            Some(r"E:\OneDrive\WorkBuddy"),
        )
        .expect("rewrite");

        assert_eq!(
            rewrite_workspace_path(r"E:\OneDrive\WorkBuddy\Project", &rewrite),
            None
        );
        assert_eq!(rewrite_workspace_path(r"relative\Project", &rewrite), None);

        let cross_device = workspace_path_rewrite(
            Some(r"D:\OneDrive\WorkBuddy"),
            Some(r"E:\OneDrive\WorkBuddy"),
        )
        .expect("cross-device rewrite");
        assert_eq!(
            rewrite_workspace_path(
                r"D:\WorkSpace\fanruan\fine-passport\fine-passport-springboot",
                &cross_device,
            ),
            Some(r"E:\WorkSpace\fanruan\fine-passport\fine-passport-springboot".to_string())
        );
        assert_eq!(
            rewrite_workspace_path(r"C:\Users\PC\project", &cross_device),
            None
        );
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
