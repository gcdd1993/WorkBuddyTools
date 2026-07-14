use crate::{
    crypto::{decrypt_package, encrypt_package},
    sync::{apply_sync_package, build_sync_package, SyncApplyResult, SyncStrategy},
    workbuddy_dir,
};
use chrono::Utc;
use rand::{rngs::OsRng, RngCore};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs, time::Duration};
use tempfile::tempdir;
use url::Url;

const PROTOCOL_VERSION: &str = "v1";
const PLAIN_ZIP_NAME: &str = "workbuddy-sync.zip";
const ENCRYPTED_ZIP_NAME: &str = "workbuddy-sync.zip.enc";
const PUBLIC_MANIFEST_NAME: &str = "manifest.public.json";
const LATEST_MANIFEST_NAME: &str = "latest.json";
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const TRANSFER_TIMEOUT_SECS: u64 = 180;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebDavSyncSettings {
    pub base_url: String,
    pub username: String,
    pub password: String,
    #[serde(default = "default_remote_root")]
    pub remote_root: String,
    #[serde(default = "default_profile")]
    pub profile: String,
    pub passphrase: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebDavPublicManifest {
    pub schema_version: u32,
    pub generation: String,
    pub created_at: String,
    pub device_name: String,
    pub encrypted_package_path: String,
    pub encrypted_package_sha256: String,
    pub encrypted_package_size: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebDavRemoteInfo {
    pub generation: String,
    pub created_at: String,
    pub device_name: String,
    pub encrypted_package_path: String,
    pub encrypted_package_sha256: String,
    pub encrypted_package_size: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebDavSyncResult {
    pub status: String,
    pub strategy: SyncStrategy,
    pub generation: Option<String>,
    pub remote_path: Option<String>,
    pub backup_dir: Option<String>,
    pub conflicts: Vec<String>,
    pub message: String,
}

impl WebDavSyncSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.base_url.trim().is_empty() {
            return Err("WebDAV 地址不能为空".to_string());
        }
        let parsed =
            Url::parse(self.base_url.trim()).map_err(|err| format!("WebDAV 地址无效：{err}"))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err("WebDAV 地址必须使用 http 或 https".to_string());
        }
        if self.username.trim().is_empty() {
            return Err("WebDAV 用户名不能为空".to_string());
        }
        if self.password.is_empty() {
            return Err("WebDAV 密码不能为空".to_string());
        }
        Ok(())
    }

    fn remote_root(&self) -> String {
        let trimmed = self.remote_root.trim();
        if trimmed.is_empty() {
            default_remote_root()
        } else {
            trimmed.to_string()
        }
    }
}

fn default_remote_root() -> String {
    "WorkBuddySync".to_string()
}

fn default_profile() -> String {
    "default".to_string()
}

#[tauri::command]
pub async fn webdav_test_connection(settings: WebDavSyncSettings) -> Result<(), String> {
    settings.validate()?;
    if propfind_exists(&settings, &[]).await? {
        Ok(())
    } else {
        Err("WebDAV 连接失败：服务器未返回成功状态".to_string())
    }
}

#[tauri::command]
pub async fn webdav_fetch_remote_info(
    settings: WebDavSyncSettings,
) -> Result<Option<WebDavRemoteInfo>, String> {
    settings.validate()?;
    let Some(manifest) = fetch_latest_manifest(&settings).await? else {
        return Ok(None);
    };
    Ok(Some(WebDavRemoteInfo {
        generation: manifest.generation,
        created_at: manifest.created_at,
        device_name: manifest.device_name,
        encrypted_package_path: manifest.encrypted_package_path,
        encrypted_package_sha256: manifest.encrypted_package_sha256,
        encrypted_package_size: manifest.encrypted_package_size,
    }))
}

#[tauri::command]
pub async fn webdav_upload_sync(
    settings: WebDavSyncSettings,
    strategy: SyncStrategy,
) -> Result<WebDavSyncResult, String> {
    settings.validate()?;
    publish_local_snapshot(&settings, strategy).await
}

#[tauri::command]
pub async fn webdav_download_sync(
    settings: WebDavSyncSettings,
    strategy: SyncStrategy,
) -> Result<WebDavSyncResult, String> {
    settings.validate()?;
    if strategy == SyncStrategy::LocalOverwriteRemote {
        return publish_local_snapshot(&settings, strategy).await;
    }
    let apply = download_and_apply_snapshot(&settings, strategy).await?;
    Ok(WebDavSyncResult {
        status: "downloaded".to_string(),
        strategy,
        generation: None,
        remote_path: None,
        backup_dir: apply
            .backup_dir
            .map(|path| path.to_string_lossy().to_string()),
        conflicts: apply.conflicts,
        message: match strategy {
            SyncStrategy::SmartMerge => "已从远端下载 ZIP，并与本机配置智能合并".to_string(),
            SyncStrategy::RemoteOverwriteLocal => {
                "已使用远端 ZIP 覆盖本机会话和模型配置".to_string()
            }
            SyncStrategy::LocalOverwriteRemote => unreachable!(),
        },
    })
}

#[tauri::command]
pub async fn webdav_run_sync(
    settings: WebDavSyncSettings,
    strategy: SyncStrategy,
) -> Result<WebDavSyncResult, String> {
    settings.validate()?;
    match strategy {
        SyncStrategy::LocalOverwriteRemote => publish_local_snapshot(&settings, strategy).await,
        SyncStrategy::RemoteOverwriteLocal => webdav_download_sync(settings, strategy).await,
        SyncStrategy::SmartMerge => {
            let apply = if fetch_latest_manifest(&settings).await?.is_some() {
                Some(download_and_apply_snapshot(&settings, strategy).await?)
            } else {
                None
            };
            let mut result = publish_local_snapshot(&settings, strategy).await?;
            if let Some(apply) = apply {
                result.backup_dir = apply
                    .backup_dir
                    .map(|path| path.to_string_lossy().to_string());
                result.conflicts = apply.conflicts;
                result.message = if result.conflicts.is_empty() {
                    if settings.passphrase.trim().is_empty() {
                        "已合并远端 ZIP，并将合并后的本机快照以明文 ZIP 上传".to_string()
                    } else {
                        "已合并远端 ZIP，并将合并后的本机快照加密上传".to_string()
                    }
                } else {
                    format!(
                        "已合并远端 ZIP，并上传新快照；生成 {} 个会话冲突副本",
                        result.conflicts.len()
                    )
                };
            }
            Ok(result)
        }
    }
}

async fn publish_local_snapshot(
    settings: &WebDavSyncSettings,
    strategy: SyncStrategy,
) -> Result<WebDavSyncResult, String> {
    let workbuddy_dir = workbuddy_dir()?;
    let tmp = tempdir().map_err(|err| format!("创建同步临时目录失败：{err}"))?;
    let plain_zip = tmp.path().join(PLAIN_ZIP_NAME);
    let encrypted_zip = tmp.path().join(ENCRYPTED_ZIP_NAME);
    build_sync_package(&workbuddy_dir, &plain_zip)?;
    let encrypted = !settings.passphrase.trim().is_empty();
    let (package_name, package_bytes) = if encrypted {
        encrypt_package(&plain_zip, &encrypted_zip, &settings.passphrase)?;
        (
            ENCRYPTED_ZIP_NAME,
            fs::read(&encrypted_zip).map_err(|err| format!("读取加密 ZIP 失败：{err}"))?,
        )
    } else {
        (
            PLAIN_ZIP_NAME,
            fs::read(&plain_zip).map_err(|err| format!("读取明文 ZIP 失败：{err}"))?,
        )
    };
    let now = Utc::now();
    let generation = format!(
        "{}-{:016x}",
        now.format("%Y%m%dT%H%M%S%.9fZ"),
        OsRng.next_u64()
    );
    let package_path = remote_generation_path(settings, &generation, package_name);
    let manifest_path = remote_generation_manifest_path(settings, &generation);
    let latest_path = remote_latest_manifest_path(settings);

    ensure_remote_directories(settings, &generation).await?;
    put_bytes(
        settings,
        &path_segments(&package_path),
        package_bytes.clone(),
        "application/zip",
    )
    .await?;

    let public_manifest = WebDavPublicManifest {
        schema_version: 1,
        generation: generation.clone(),
        created_at: now.to_rfc3339(),
        device_name: device_name(),
        encrypted_package_path: package_path.clone(),
        encrypted_package_sha256: sha256_hex(&package_bytes),
        encrypted_package_size: package_bytes.len() as u64,
    };
    let manifest_bytes = serde_json::to_vec_pretty(&public_manifest)
        .map_err(|err| format!("序列化远端同步清单失败：{err}"))?;
    put_bytes(
        settings,
        &path_segments(&manifest_path),
        manifest_bytes.clone(),
        "application/json",
    )
    .await?;
    put_bytes(
        settings,
        &path_segments(&latest_path),
        manifest_bytes,
        "application/json",
    )
    .await?;

    Ok(WebDavSyncResult {
        status: "uploaded".to_string(),
        strategy,
        generation: Some(generation),
        remote_path: Some(package_path),
        backup_dir: None,
        conflicts: Vec::new(),
        message: if encrypted {
            "已打包、加密并上传 WorkBuddy 会话和模型配置 ZIP".to_string()
        } else {
            "已打包并以明文 ZIP 上传 WorkBuddy 会话和模型配置".to_string()
        },
    })
}

async fn download_and_apply_snapshot(
    settings: &WebDavSyncSettings,
    strategy: SyncStrategy,
) -> Result<SyncApplyResult, String> {
    let manifest = fetch_latest_manifest(settings)
        .await?
        .ok_or_else(|| "远端没有可下载的 WorkBuddy 同步包".to_string())?;
    validate_remote_package_path(settings, &manifest.encrypted_package_path)?;
    let package_bytes = get_bytes(
        settings,
        &path_segments(&manifest.encrypted_package_path),
        512 * 1024 * 1024,
    )
    .await?
    .ok_or_else(|| "远端同步包不存在".to_string())?;
    let actual_hash = sha256_hex(&package_bytes);
    if actual_hash != manifest.encrypted_package_sha256 {
        return Err(format!(
            "远端同步包 SHA-256 校验失败：expected {}, got {}",
            manifest.encrypted_package_sha256, actual_hash
        ));
    }
    if package_bytes.len() as u64 != manifest.encrypted_package_size {
        return Err(format!(
            "远端同步包大小校验失败：expected {}, got {}",
            manifest.encrypted_package_size,
            package_bytes.len()
        ));
    }

    let tmp = tempdir().map_err(|err| format!("创建同步临时目录失败：{err}"))?;
    let encrypted_path = tmp.path().join(ENCRYPTED_ZIP_NAME);
    let plain_zip = tmp.path().join(PLAIN_ZIP_NAME);
    let package_name = path_segments(&manifest.encrypted_package_path)
        .last()
        .cloned()
        .ok_or_else(|| "远端同步清单缺少同步包文件名".to_string())?;
    if package_name == ENCRYPTED_ZIP_NAME {
        if settings.passphrase.trim().is_empty() {
            return Err("远端同步包已加密，请填写同步加密密码".to_string());
        }
        fs::write(&encrypted_path, package_bytes)
            .map_err(|err| format!("写入临时加密包失败：{err}"))?;
        decrypt_package(&encrypted_path, &plain_zip, &settings.passphrase)?;
    } else {
        fs::write(&plain_zip, package_bytes)
            .map_err(|err| format!("写入临时明文 ZIP 失败：{err}"))?;
    }
    apply_sync_package(&workbuddy_dir()?, &plain_zip, strategy)
}

async fn fetch_latest_manifest(
    settings: &WebDavSyncSettings,
) -> Result<Option<WebDavPublicManifest>, String> {
    let latest = remote_latest_manifest_path(settings);
    let Some(bytes) = get_bytes(settings, &path_segments(&latest), 1024 * 1024).await? else {
        return Ok(None);
    };
    let manifest: WebDavPublicManifest =
        serde_json::from_slice(&bytes).map_err(|err| format!("解析远端同步清单失败：{err}"))?;
    Ok(Some(manifest))
}

pub(crate) fn remote_generation_path(
    settings: &WebDavSyncSettings,
    generation: &str,
    package_name: &str,
) -> String {
    format!(
        "{}/{PROTOCOL_VERSION}/generations/{generation}/{package_name}",
        settings.remote_root()
    )
}

fn remote_generation_manifest_path(settings: &WebDavSyncSettings, generation: &str) -> String {
    format!(
        "{}/{PROTOCOL_VERSION}/generations/{generation}/{PUBLIC_MANIFEST_NAME}",
        settings.remote_root()
    )
}

fn remote_latest_manifest_path(settings: &WebDavSyncSettings) -> String {
    format!(
        "{}/{PROTOCOL_VERSION}/{LATEST_MANIFEST_NAME}",
        settings.remote_root()
    )
}

fn validate_remote_package_path(
    settings: &WebDavSyncSettings,
    package_path: &str,
) -> Result<(), String> {
    let segments = path_segments(package_path);
    let root_segments = path_segments(&settings.remote_root());
    let has_expected_prefix = segments.starts_with(&root_segments)
        && segments.get(root_segments.len()).map(String::as_str) == Some(PROTOCOL_VERSION)
        && segments.get(root_segments.len() + 1).map(String::as_str) == Some("generations")
        && segments.len() == root_segments.len() + 4
        && matches!(
            segments.last().map(String::as_str),
            Some(ENCRYPTED_ZIP_NAME) | Some(PLAIN_ZIP_NAME)
        );
    if has_expected_prefix && !segments.iter().any(|segment| segment == "..") {
        Ok(())
    } else {
        Err("远端同步清单包含无效的加密包路径".to_string())
    }
}

async fn ensure_remote_directories(
    settings: &WebDavSyncSettings,
    generation: &str,
) -> Result<(), String> {
    let dirs = [
        settings.remote_root(),
        format!("{}/{PROTOCOL_VERSION}", settings.remote_root()),
        format!("{}/{PROTOCOL_VERSION}/generations", settings.remote_root()),
        format!(
            "{}/{PROTOCOL_VERSION}/generations/{generation}",
            settings.remote_root()
        ),
    ];
    for dir in dirs {
        mkcol_if_missing(settings, &path_segments(&dir)).await?;
    }
    Ok(())
}

async fn propfind_exists(
    settings: &WebDavSyncSettings,
    segments: &[String],
) -> Result<bool, String> {
    let client = reqwest::Client::new();
    let url = build_remote_url(&settings.base_url, segments)?;
    let response = client
        .request(method_propfind()?, url)
        .basic_auth(settings.username.trim(), Some(settings.password.as_str()))
        .header("Depth", "0")
        .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|err| format!("WebDAV PROPFIND 请求失败：{err}"))?;
    Ok(response.status().is_success())
}

async fn mkcol_if_missing(
    settings: &WebDavSyncSettings,
    segments: &[String],
) -> Result<(), String> {
    let client = reqwest::Client::new();
    let url = build_remote_url(&settings.base_url, segments)?;
    let response = client
        .request(method_mkcol()?, url.clone())
        .basic_auth(settings.username.trim(), Some(settings.password.as_str()))
        .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|err| format!("WebDAV MKCOL 请求失败：{err}"))?;
    let status = response.status();
    if status.is_success() || status == StatusCode::CREATED {
        return Ok(());
    }
    if matches!(
        status,
        StatusCode::METHOD_NOT_ALLOWED | StatusCode::CONFLICT
    ) && propfind_exists(settings, segments).await?
    {
        return Ok(());
    }
    Err(format!(
        "WebDAV MKCOL 失败：{} {}",
        status.as_u16(),
        redact_url(&url)
    ))
}

async fn put_bytes(
    settings: &WebDavSyncSettings,
    segments: &[String],
    bytes: Vec<u8>,
    content_type: &str,
) -> Result<(), String> {
    let client = reqwest::Client::new();
    let url = build_remote_url(&settings.base_url, segments)?;
    let response = client
        .put(url.clone())
        .basic_auth(settings.username.trim(), Some(settings.password.as_str()))
        .header("Content-Type", content_type)
        .body(bytes)
        .timeout(Duration::from_secs(TRANSFER_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|err| format!("WebDAV PUT 请求失败：{err}"))?;
    if response.status().is_success() {
        return Ok(());
    }
    Err(format!(
        "WebDAV PUT 失败：{} {}",
        response.status().as_u16(),
        redact_url(&url)
    ))
}

async fn get_bytes(
    settings: &WebDavSyncSettings,
    segments: &[String],
    max_bytes: usize,
) -> Result<Option<Vec<u8>>, String> {
    let client = reqwest::Client::new();
    let url = build_remote_url(&settings.base_url, segments)?;
    let response = client
        .get(url.clone())
        .basic_auth(settings.username.trim(), Some(settings.password.as_str()))
        .timeout(Duration::from_secs(TRANSFER_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|err| format!("WebDAV GET 请求失败：{err}"))?;
    if response.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(format!(
            "WebDAV GET 失败：{} {}",
            response.status().as_u16(),
            redact_url(&url)
        ));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|err| format!("读取 WebDAV 响应失败：{err}"))?;
    if bytes.len() > max_bytes {
        return Err(format!("WebDAV 响应超过大小限制：{} bytes", bytes.len()));
    }
    Ok(Some(bytes.to_vec()))
}

fn build_remote_url(base_url: &str, segments: &[String]) -> Result<Url, String> {
    let mut url = Url::parse(base_url.trim()).map_err(|err| format!("WebDAV 地址无效：{err}"))?;
    let normalized_path = url.path().trim_end_matches('/').to_string();
    url.set_path(if normalized_path.is_empty() {
        "/"
    } else {
        &normalized_path
    });
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| "WebDAV 地址不能作为基础 URL".to_string())?;
        path.pop_if_empty();
        for segment in segments {
            path.push(segment);
        }
    }
    Ok(url)
}

fn path_segments(path: &str) -> Vec<String> {
    path.replace('\\', "/")
        .split('/')
        .filter_map(|segment| {
            let trimmed = segment.trim();
            if trimmed.is_empty() || trimmed == "." {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .collect()
}

fn method_propfind() -> Result<Method, String> {
    Method::from_bytes(b"PROPFIND").map_err(|err| format!("创建 PROPFIND 方法失败：{err}"))
}

fn method_mkcol() -> Result<Method, String> {
    Method::from_bytes(b"MKCOL").map_err(|err| format!("创建 MKCOL 方法失败：{err}"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "Unknown Device".to_string())
}

fn redact_url(url: &Url) -> String {
    let mut redacted = url.clone();
    let _ = redacted.set_username("");
    let _ = redacted.set_password(None);
    redacted.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::to_string;

    #[test]
    fn remote_generation_path_uses_versioned_encrypted_zip_layout() {
        let settings = WebDavSyncSettings {
            base_url: "https://dav.example.com/remote.php/dav/files/me".to_string(),
            username: "demo".to_string(),
            password: "secret".to_string(),
            remote_root: "WorkBuddySync".to_string(),
            profile: "default".to_string(),
            passphrase: "sync-secret".to_string(),
        };

        let path = remote_generation_path(&settings, "000001", ENCRYPTED_ZIP_NAME);

        assert_eq!(
            path,
            "WorkBuddySync/v1/generations/000001/workbuddy-sync.zip.enc"
        );
    }

    #[test]
    fn webdav_settings_validation_rejects_missing_url_or_credentials() {
        let mut settings = WebDavSyncSettings {
            base_url: String::new(),
            username: "demo".to_string(),
            password: "secret".to_string(),
            remote_root: "WorkBuddySync".to_string(),
            profile: "default".to_string(),
            passphrase: "sync-secret".to_string(),
        };
        assert!(settings.validate().is_err());

        settings.base_url = "https://dav.example.com".to_string();
        settings.username.clear();
        assert!(settings.validate().is_err());

        settings.username = "demo".to_string();
        settings.password.clear();
        assert!(settings.validate().is_err());

        settings.password = "secret".to_string();
        settings.passphrase.clear();
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn public_manifest_does_not_include_api_keys_or_local_paths() {
        let manifest = WebDavPublicManifest {
            schema_version: 1,
            generation: "000001".to_string(),
            created_at: "2026-07-12T00:00:00Z".to_string(),
            device_name: "dev-box".to_string(),
            encrypted_package_path: "WorkBuddySync/v1/generations/000001/workbuddy-sync.zip.enc"
                .to_string(),
            encrypted_package_sha256: "abc123".to_string(),
            encrypted_package_size: 42,
        };

        let serialized = to_string(&manifest).expect("serialize manifest");

        assert!(!serialized.contains("sk-"));
        assert!(!serialized.contains("apiKey"));
        assert!(!serialized.contains("C:\\"));
        assert!(serialized.contains("workbuddy-sync.zip.enc"));
    }
}
