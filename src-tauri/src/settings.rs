use serde::{Deserialize, Serialize};
use std::{env, fs, path::{Path, PathBuf}};

const APP_CONFIG_DIR: &str = "com.workbuddytools.modelconfig";
const WORKBUDDY_TOOLS_DIR: &str = "workbuddy-tools";
const SETTINGS_FILE: &str = "settings.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebDavSettings {
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default = "default_remote_root")]
    pub remote_root: String,
    #[serde(default)]
    pub passphrase: String,
}

impl Default for WebDavSettings {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            username: String::new(),
            password: String::new(),
            remote_root: default_remote_root(),
            passphrase: String::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    #[serde(default)]
    pub webdav: WebDavSettings,
}

fn default_remote_root() -> String {
    "WorkBuddySync".to_string()
}

pub fn settings_path() -> Result<PathBuf, String> {
    let user_profile = env::var_os("USERPROFILE")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "无法读取 USERPROFILE 环境变量，不能确定本机设置目录".to_string())?;
    Ok(PathBuf::from(user_profile)
        .join(".workbuddy")
        .join(WORKBUDDY_TOOLS_DIR)
        .join(SETTINGS_FILE))
}

pub fn read_app_settings() -> Result<AppSettings, String> {
    let path = settings_path()?;
    if !path.exists() {
        migrate_legacy_settings(&path)?;
        if !path.exists() {
            return Ok(AppSettings::default());
        }
    }
    let content = fs::read_to_string(&path)
        .map_err(|err| format!("读取应用设置 {} 失败：{err}", path.display()))?;
    serde_json::from_str(&content)
        .map_err(|err| format!("解析应用设置 {} 失败：{err}。原文件未被修改", path.display()))
}

#[tauri::command]
pub fn load_app_settings() -> Result<AppSettings, String> {
    read_app_settings()
}

#[tauri::command]
pub fn save_app_settings(mut settings: AppSettings) -> Result<AppSettings, String> {
    normalize_settings(&mut settings);
    let path = settings_path()?;
    write_settings(&path, &settings)?;
    Ok(settings)
}

fn write_settings(path: &Path, settings: &AppSettings) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| "应用设置路径无父目录".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|err| format!("创建应用设置目录 {} 失败：{err}", parent.display()))?;

    let serialized = serde_json::to_string_pretty(&settings)
        .map_err(|err| format!("序列化应用设置失败：{err}"))?;
    let temp_path = path.with_extension(format!("json.{}.tmp", std::process::id()));
    fs::write(&temp_path, format!("{serialized}\n"))
        .map_err(|err| format!("写入临时设置文件 {} 失败：{err}", temp_path.display()))?;

    replace_file(&temp_path, &path)?;
    Ok(())
}

fn normalize_settings(settings: &mut AppSettings) {
    settings.webdav.base_url = settings.webdav.base_url.trim().to_string();
    settings.webdav.username = settings.webdav.username.trim().to_string();
    settings.webdav.remote_root = settings.webdav.remote_root.trim().to_string();
    if settings.webdav.remote_root.is_empty() {
        settings.webdav.remote_root = default_remote_root();
    }
}

fn migrate_legacy_settings(new_path: &Path) -> Result<(), String> {
    let Some(app_data) = env::var_os("APPDATA").filter(|value| !value.is_empty()) else {
        return Ok(());
    };
    let legacy_path = PathBuf::from(app_data).join(APP_CONFIG_DIR).join(SETTINGS_FILE);
    if !legacy_path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&legacy_path)
        .map_err(|err| format!("读取旧应用设置 {} 失败：{err}", legacy_path.display()))?;
    let mut settings: AppSettings = serde_json::from_str(&content)
        .map_err(|err| format!("解析旧应用设置 {} 失败：{err}。原文件未被修改", legacy_path.display()))?;
    normalize_settings(&mut settings);
    write_settings(new_path, &settings)?;
    fs::remove_file(&legacy_path)
        .map_err(|err| format!("新设置已保存，但删除旧应用设置 {} 失败：{err}", legacy_path.display()))?;
    Ok(())
}

fn replace_file(temp_path: &Path, target_path: &Path) -> Result<(), String> {
    if !target_path.exists() {
        return fs::rename(temp_path, target_path)
            .map_err(|err| format!("保存应用设置 {} 失败：{err}", target_path.display()));
    }

    let backup_path = target_path.with_extension("json.previous");
    if backup_path.exists() {
        fs::remove_file(&backup_path)
            .map_err(|err| format!("清理旧设置备份 {} 失败：{err}", backup_path.display()))?;
    }
    fs::rename(target_path, &backup_path)
        .map_err(|err| format!("准备替换应用设置 {} 失败：{err}", target_path.display()))?;
    match fs::rename(temp_path, target_path) {
        Ok(()) => {
            let _ = fs::remove_file(backup_path);
            Ok(())
        }
        Err(err) => {
            let _ = fs::rename(&backup_path, target_path);
            Err(format!("替换应用设置 {} 失败：{err}", target_path.display()))
        }
    }
}
