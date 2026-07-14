pub mod crypto;
pub mod sessions;
pub mod settings;
pub mod sync;
pub mod webdav;

use chrono::{DateTime, Utc};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::OnceLock;
use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
    time::Duration,
};
use url::Url;

const MODELS_FILE_NAME: &str = "models.json";
const PROVIDERS_FILE_NAME: &str = "model-providers.json";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppPaths {
    workbuddy_dir: String,
    models_file: String,
    providers_file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Provider {
    id: String,
    name: String,
    base_url: String,
    api_key: String,
    created_at: Option<DateTime<Utc>>,
    updated_at: Option<DateTime<Utc>>,
    last_fetched_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderInput {
    id: Option<String>,
    name: String,
    base_url: String,
    api_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelCapabilities {
    supports_tool_call: bool,
    supports_images: bool,
    supports_reasoning: bool,
    use_custom_protocol: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderModel {
    id: String,
    name: String,
    provider_id: String,
    provider_name: String,
    max_input_tokens: Option<u64>,
    max_output_tokens: Option<u64>,
    raw: Value,
    capabilities: ModelCapabilities,
}

#[derive(Debug, Clone)]
struct ModelDatabaseInfo {
    context_window: Option<u64>,
    max_output: Option<u64>,
    supports_tool_call: Option<bool>,
    supports_images: Option<bool>,
    supports_reasoning: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FetchModelsResult {
    provider: Provider,
    models: Vec<ProviderModel>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AddModelsResult {
    models: Vec<Value>,
    added: usize,
    updated: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddModelsPayload {
    provider_id: String,
    model_ids: Vec<String>,
    fetched_models: Vec<ProviderModel>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModelsResponse {
    data: Vec<Value>,
}

#[tauri::command]
fn get_paths() -> Result<AppPaths, String> {
    let workbuddy_dir = workbuddy_dir()?;
    Ok(AppPaths {
        models_file: path_to_string(&workbuddy_dir.join(MODELS_FILE_NAME)),
        providers_file: path_to_string(&workbuddy_dir.join(PROVIDERS_FILE_NAME)),
        workbuddy_dir: path_to_string(&workbuddy_dir),
    })
}

#[tauri::command]
fn load_workbuddy_models() -> Result<Vec<Value>, String> {
    read_workbuddy_models()
}

#[tauri::command]
fn delete_workbuddy_model(model_id: String) -> Result<Vec<Value>, String> {
    let model_id = required_trimmed(model_id, "模型 ID")?;
    let mut models = read_workbuddy_models()?;
    let removed = remove_model_by_id(&mut models, &model_id);

    if removed == 0 {
        return Err("未找到要删除的模型".to_string());
    }

    write_workbuddy_models(&models)?;
    Ok(models)
}

#[tauri::command]
fn load_providers() -> Result<Vec<Provider>, String> {
    read_providers()
}

#[tauri::command]
fn save_provider(input: ProviderInput) -> Result<Vec<Provider>, String> {
    let mut providers = read_providers()?;
    let now = Utc::now();
    let id = input
        .id
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| provider_id_from_name(&input.name));

    let normalized_provider = Provider {
        id: id.clone(),
        name: required_trimmed(input.name, "供应商名称")?,
        base_url: normalize_base_url(&required_trimmed(input.base_url, "API 请求地址")?)?,
        api_key: required_trimmed(input.api_key, "API Key")?,
        created_at: None,
        updated_at: Some(now),
        last_fetched_at: None,
    };

    if let Some(existing) = providers.iter_mut().find(|provider| provider.id == id) {
        let created_at = existing.created_at.or(Some(now));
        let last_fetched_at = existing.last_fetched_at;
        *existing = Provider {
            created_at,
            last_fetched_at,
            ..normalized_provider
        };
    } else {
        providers.push(Provider {
            created_at: Some(now),
            ..normalized_provider
        });
    }

    write_providers(&providers)?;
    Ok(providers)
}

#[tauri::command]
fn delete_provider(provider_id: String) -> Result<Vec<Provider>, String> {
    let mut providers = read_providers()?;
    let original_len = providers.len();
    providers.retain(|provider| provider.id != provider_id);

    if providers.len() == original_len {
        return Err("未找到要删除的供应商".to_string());
    }

    write_providers(&providers)?;
    Ok(providers)
}

#[tauri::command]
async fn fetch_provider_models(provider_id: String) -> Result<FetchModelsResult, String> {
    let mut providers = read_providers()?;
    let provider_position = providers
        .iter()
        .position(|provider| provider.id == provider_id)
        .ok_or_else(|| "未找到供应商".to_string())?;
    let provider = providers[provider_position].clone();
    let models_url = models_endpoint(&provider.base_url)?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(|err| format!("创建 HTTP 客户端失败：{err}"))?;

    let response = client
        .get(models_url.clone())
        .bearer_auth(&provider.api_key)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|err| format!("请求 {models_url} 失败：{err}"))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|err| format!("读取模型响应失败：{err}"))?;

    if !status.is_success() {
        return Err(format_model_fetch_error(status, &body));
    }

    let parsed: OpenAiModelsResponse = serde_json::from_str(&body)
        .map_err(|err| format!("模型响应不是 OpenAI 兼容格式：{err}"))?;

    let mut models = parsed
        .data
        .into_iter()
        .filter_map(|raw| {
            let id = raw.get("id")?.as_str()?.trim().to_string();
            if id.is_empty() {
                return None;
            }

            let database_info = model_database_info(&id);
            Some(ProviderModel {
                name: id.clone(),
                capabilities: infer_capabilities(&id, &raw, database_info.as_ref()),
                max_input_tokens: extract_max_input_tokens(&raw)
                    .or_else(|| database_info.as_ref().and_then(|info| info.context_window)),
                max_output_tokens: extract_max_output_tokens(&raw)
                    .or_else(|| database_info.as_ref().and_then(|info| info.max_output)),
                id,
                provider_id: provider.id.clone(),
                provider_name: provider.name.clone(),
                raw,
            })
        })
        .collect::<Vec<_>>();

    models.sort_by(|left, right| left.id.cmp(&right.id));

    providers[provider_position].last_fetched_at = Some(Utc::now());
    providers[provider_position].updated_at = Some(Utc::now());
    write_providers(&providers)?;

    Ok(FetchModelsResult {
        provider: providers[provider_position].clone(),
        models,
    })
}

#[tauri::command]
fn add_models_to_workbuddy(payload: AddModelsPayload) -> Result<AddModelsResult, String> {
    if payload.model_ids.is_empty() {
        return Err("请选择至少一个模型".to_string());
    }

    let providers = read_providers()?;
    let provider = providers
        .into_iter()
        .find(|item| item.id == payload.provider_id)
        .ok_or_else(|| "未找到供应商".to_string())?;

    let fetched_by_id = payload
        .fetched_models
        .into_iter()
        .map(|model| (model.id.clone(), model))
        .collect::<HashMap<_, _>>();

    let mut models = read_workbuddy_models()?;
    let mut added = 0;
    let mut updated = 0;

    for model_id in payload.model_ids {
        let fetched = fetched_by_id
            .get(&model_id)
            .ok_or_else(|| format!("缺少已拉取模型信息：{model_id}"))?;
        let workbuddy_model = workbuddy_model_from_provider(&provider, fetched)?;

        if let Some(existing) = models
            .iter_mut()
            .find(|model| model.get("id").and_then(Value::as_str) == Some(model_id.as_str()))
        {
            merge_model_object(existing, workbuddy_model)?;
            updated += 1;
        } else {
            models.push(workbuddy_model);
            added += 1;
        }
    }

    write_workbuddy_models(&models)?;
    Ok(AddModelsResult {
        models,
        added,
        updated,
    })
}

fn read_workbuddy_models() -> Result<Vec<Value>, String> {
    let models_file = workbuddy_dir()?.join(MODELS_FILE_NAME);

    if !models_file.exists() {
        write_json_file(&models_file, &Vec::<Value>::new())?;
        return Ok(Vec::new());
    }

    let content = fs::read_to_string(&models_file)
        .map_err(|err| format!("读取 WorkBuddy 模型配置失败：{err}"))?;

    if content.trim().is_empty() {
        return Ok(Vec::new());
    }

    serde_json::from_str::<Vec<Value>>(&content)
        .map_err(|err| format!("解析 WorkBuddy 模型配置失败：{err}"))
}

fn write_workbuddy_models(models: &[Value]) -> Result<(), String> {
    let models_file = workbuddy_dir()?.join(MODELS_FILE_NAME);
    backup_file(&models_file)?;
    write_json_file(&models_file, models)
}

fn read_providers() -> Result<Vec<Provider>, String> {
    let providers_file = workbuddy_dir()?.join(PROVIDERS_FILE_NAME);

    if !providers_file.exists() {
        write_json_file(&providers_file, &Vec::<Provider>::new())?;
        return Ok(Vec::new());
    }

    let content =
        fs::read_to_string(&providers_file).map_err(|err| format!("读取供应商配置失败：{err}"))?;

    if content.trim().is_empty() {
        return Ok(Vec::new());
    }

    serde_json::from_str::<Vec<Provider>>(&content)
        .map_err(|err| format!("解析供应商配置失败：{err}"))
}

fn write_providers(providers: &[Provider]) -> Result<(), String> {
    let providers_file = workbuddy_dir()?.join(PROVIDERS_FILE_NAME);
    write_json_file(&providers_file, providers)
}

fn write_json_file<T: Serialize + ?Sized>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("创建目录失败：{err}"))?;
    }

    let serialized =
        serde_json::to_string_pretty(value).map_err(|err| format!("序列化 JSON 失败：{err}"))?;
    fs::write(path, format!("{serialized}\n")).map_err(|err| format!("写入文件失败：{err}"))
}

fn backup_file(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }

    let timestamp = Utc::now().format("%Y%m%dT%H%M%SZ");
    let backup_path = path.with_file_name(format!("{MODELS_FILE_NAME}.{timestamp}.bak"));
    fs::copy(path, backup_path).map_err(|err| format!("备份 WorkBuddy 模型配置失败：{err}"))?;
    Ok(())
}

pub(crate) fn workbuddy_dir() -> Result<PathBuf, String> {
    let user_profile =
        env::var("USERPROFILE").map_err(|_| "无法读取 USERPROFILE 环境变量".to_string())?;
    Ok(PathBuf::from(user_profile).join(".workbuddy"))
}

fn provider_id_from_name(name: &str) -> String {
    let normalized = name
        .trim()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    let collapsed = normalized
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    if collapsed.is_empty() {
        format!("provider-{}", Utc::now().timestamp())
    } else {
        collapsed
    }
}

fn required_trimmed(value: String, label: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err(format!("{label}不能为空"))
    } else {
        Ok(trimmed.to_string())
    }
}

fn normalize_base_url(value: &str) -> Result<String, String> {
    let parsed = Url::parse(value).map_err(|err| format!("API 请求地址无效：{err}"))?;

    match parsed.scheme() {
        "http" | "https" => {}
        _ => return Err("API 请求地址必须使用 http 或 https".to_string()),
    }

    Ok(value.trim_end_matches('/').to_string())
}

fn models_endpoint(base_url: &str) -> Result<Url, String> {
    let base = normalize_openai_base(base_url)?;
    base.join("models")
        .map_err(|err| format!("拼接 /v1/models 失败：{err}"))
}

fn chat_completions_endpoint(base_url: &str) -> Result<String, String> {
    let base = normalize_openai_base(base_url)?;
    base.join("chat/completions")
        .map(|url| url.to_string())
        .map_err(|err| format!("拼接 /v1/chat/completions 失败：{err}"))
}

fn normalize_openai_base(base_url: &str) -> Result<Url, String> {
    let parsed = Url::parse(base_url).map_err(|err| format!("API 请求地址无效：{err}"))?;
    let mut segments = parsed
        .path_segments()
        .map(|items| {
            items
                .filter(|item| !item.is_empty())
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if let Some(position) = segments.iter().position(|segment| segment == "v1") {
        segments.truncate(position + 1);
    } else {
        segments.push("v1".to_string());
    }

    let mut normalized = parsed;
    normalized.set_path(&format!("{}/", segments.join("/")));
    normalized.set_query(None);
    normalized.set_fragment(None);
    Ok(normalized)
}

fn infer_capabilities(
    id: &str,
    raw: &Value,
    database_info: Option<&ModelDatabaseInfo>,
) -> ModelCapabilities {
    let lower_id = id.to_ascii_lowercase();
    let family = raw
        .get("owned_by")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let searchable = format!("{lower_id} {family}");

    let supports_images = contains_any(
        &searchable,
        &[
            "vision",
            "visual",
            "image",
            "multimodal",
            "vl",
            "gpt-4o",
            "omni",
        ],
    );
    let supports_reasoning = contains_any(
        &searchable,
        &[
            "reason",
            "thinking",
            "think",
            "r1",
            "o1",
            "o3",
            "o4",
            "deepseek-reasoner",
            "qwq",
        ],
    );
    let tool_call_blocked = contains_any(
        &searchable,
        &[
            "embedding",
            "embed",
            "rerank",
            "tts",
            "audio",
            "whisper",
            "moderation",
            "image-generation",
            "dall-e",
        ],
    );

    ModelCapabilities {
        supports_tool_call: database_info
            .and_then(|info| info.supports_tool_call)
            .unwrap_or(!tool_call_blocked),
        supports_images: database_info
            .and_then(|info| info.supports_images)
            .unwrap_or(supports_images),
        supports_reasoning: database_info
            .and_then(|info| info.supports_reasoning)
            .unwrap_or(supports_reasoning),
        use_custom_protocol: false,
    }
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

fn model_database_info(model: &str) -> Option<ModelDatabaseInfo> {
    let entry = model_database_entry(model)?;
    let capabilities = entry.get("capabilities");

    Some(ModelDatabaseInfo {
        context_window: entry
            .get("contextWindow")
            .and_then(value_to_u64)
            .filter(|tokens| *tokens > 0),
        max_output: entry
            .get("maxOutput")
            .and_then(value_to_u64)
            .filter(|tokens| *tokens > 0),
        supports_tool_call: capabilities
            .and_then(|value| value.get("functionCalling"))
            .and_then(Value::as_bool),
        supports_images: capabilities
            .and_then(|value| value.get("vision"))
            .and_then(Value::as_bool),
        supports_reasoning: capabilities
            .and_then(|value| value.get("reasoning"))
            .and_then(Value::as_bool),
    })
}

fn model_database_entries() -> Option<&'static serde_json::Map<String, Value>> {
    static MODEL_DATABASE: OnceLock<Value> = OnceLock::new();
    MODEL_DATABASE
        .get_or_init(|| {
            serde_json::from_str(include_str!("../resources/modelDatabase.json"))
                .unwrap_or(Value::Null)
        })
        .as_object()
}

fn model_database_entry(model: &str) -> Option<&'static Value> {
    let model = model.trim();
    if model.is_empty() {
        return None;
    }

    let entries = model_database_entries()?;
    let name = model.to_ascii_lowercase();
    let stripped = name.rsplit('/').next().unwrap_or(&name);

    if let Some(entry) = entries.get(name.as_str()) {
        return Some(entry);
    }
    if let Some(entry) = entries.get(stripped) {
        return Some(entry);
    }

    let candidates = if name == stripped {
        vec![stripped]
    } else {
        vec![name.as_str(), stripped]
    };

    entries
        .iter()
        .filter_map(|(key, entry)| {
            if key == "_meta"
                || !candidates
                    .iter()
                    .any(|candidate| candidate.starts_with(key) && key.len() < candidate.len())
            {
                return None;
            }
            Some((key.len(), entry))
        })
        .max_by_key(|(key_len, _)| *key_len)
        .map(|(_, entry)| entry)
        .or_else(|| {
            entries
                .iter()
                .filter_map(|(key, entry)| {
                    if key == "_meta"
                        || !candidates
                            .iter()
                            .any(|candidate| key != candidate && candidate.contains(key))
                    {
                        return None;
                    }
                    Some((key.len(), entry))
                })
                .max_by_key(|(key_len, _)| *key_len)
                .map(|(_, entry)| entry)
        })
}

fn extract_max_input_tokens(raw: &Value) -> Option<u64> {
    extract_token_limit(
        raw,
        &[
            "maxInputTokens",
            "max_input_tokens",
            "inputTokenLimit",
            "input_token_limit",
            "contextLength",
            "context_length",
            "maxContextLength",
            "max_context_length",
            "maxModelLen",
            "max_model_len",
            "modelMaxLength",
            "model_max_length",
        ],
        &[
            &["limits", "maxInputTokens"],
            &["limits", "max_input_tokens"],
            &["limits", "inputTokenLimit"],
            &["limits", "input_token_limit"],
            &["limits", "contextLength"],
            &["limits", "context_length"],
            &["metadata", "maxInputTokens"],
            &["metadata", "max_input_tokens"],
            &["metadata", "contextLength"],
            &["metadata", "context_length"],
        ],
    )
}

fn extract_max_output_tokens(raw: &Value) -> Option<u64> {
    extract_token_limit(
        raw,
        &[
            "maxOutputTokens",
            "max_output_tokens",
            "outputTokenLimit",
            "output_token_limit",
            "maxCompletionTokens",
            "max_completion_tokens",
            "maxTokens",
            "max_tokens",
        ],
        &[
            &["limits", "maxOutputTokens"],
            &["limits", "max_output_tokens"],
            &["limits", "outputTokenLimit"],
            &["limits", "output_token_limit"],
            &["limits", "maxCompletionTokens"],
            &["limits", "max_completion_tokens"],
            &["metadata", "maxOutputTokens"],
            &["metadata", "max_output_tokens"],
            &["metadata", "maxCompletionTokens"],
            &["metadata", "max_completion_tokens"],
            &["top_provider", "max_completion_tokens"],
        ],
    )
}

fn extract_token_limit(raw: &Value, direct_keys: &[&str], nested_paths: &[&[&str]]) -> Option<u64> {
    for key in direct_keys {
        if let Some(value) = raw.get(*key).and_then(value_to_u64) {
            return Some(value);
        }
    }

    for path in nested_paths {
        if let Some(value) = value_at_path(raw, path).and_then(value_to_u64) {
            return Some(value);
        }
    }

    None
}

fn value_at_path<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut cursor = value;
    for key in path {
        cursor = cursor.get(*key)?;
    }
    Some(cursor)
}

fn value_to_u64(value: &Value) -> Option<u64> {
    if let Some(number) = value.as_u64() {
        return Some(number);
    }

    if let Some(number) = value.as_f64() {
        if number.is_finite() && number >= 0.0 {
            return Some(number.floor() as u64);
        }
    }

    value.as_str().and_then(|text| {
        let normalized = text
            .trim()
            .chars()
            .filter(|character| *character != '_' && *character != ',')
            .collect::<String>();
        normalized.parse::<u64>().ok()
    })
}

fn workbuddy_model_from_provider(
    provider: &Provider,
    fetched: &ProviderModel,
) -> Result<Value, String> {
    let capabilities = &fetched.capabilities;
    let mut model = json!({
        "id": fetched.id,
        "name": format!("{}-{}", provider.name, fetched.id),
        "vendor": "Custom",
        "url": chat_completions_endpoint(&provider.base_url)?,
        "apiKey": provider.api_key,
        "supportsToolCall": capabilities.supports_tool_call,
        "supportsImages": capabilities.supports_images,
        "supportsReasoning": capabilities.supports_reasoning,
        "useCustomProtocol": capabilities.use_custom_protocol
    });

    if let Some(value) = fetched.max_input_tokens {
        model["maxInputTokens"] = json!(value);
    }

    if let Some(value) = fetched.max_output_tokens {
        model["maxOutputTokens"] = json!(value);
    }

    Ok(model)
}

fn merge_model_object(existing: &mut Value, replacement: Value) -> Result<(), String> {
    let existing_object = existing
        .as_object_mut()
        .ok_or_else(|| "WorkBuddy 模型配置中存在非对象条目，无法更新".to_string())?;
    let replacement_object = replacement
        .as_object()
        .ok_or_else(|| "新模型配置不是对象".to_string())?;

    for (key, value) in replacement_object {
        existing_object.insert(key.clone(), value.clone());
    }

    Ok(())
}

fn remove_model_by_id(models: &mut Vec<Value>, model_id: &str) -> usize {
    let original_len = models.len();
    models.retain(|model| model.get("id").and_then(Value::as_str) != Some(model_id));
    original_len - models.len()
}

fn format_model_fetch_error(status: StatusCode, body: &str) -> String {
    let body_preview = body.chars().take(500).collect::<String>();
    format!(
        "拉取模型失败，HTTP 状态码 {}：{}",
        status.as_u16(),
        body_preview
    )
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_paths,
            load_workbuddy_models,
            delete_workbuddy_model,
            load_providers,
            save_provider,
            delete_provider,
            fetch_provider_models,
            add_models_to_workbuddy,
            settings::load_app_settings,
            settings::save_app_settings,
            sessions::list_workbuddy_sessions,
            sessions::update_workbuddy_session,
            sessions::delete_workbuddy_session,
            webdav::webdav_test_connection,
            webdav::webdav_fetch_remote_info,
            webdav::webdav_upload_sync,
            webdav::webdav_download_sync,
            webdav::webdav_run_sync
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_database_matches_openrouter_style_deepseek_v4_pro() {
        let info = model_database_info("deepseek-ai/DeepSeek-V4-Pro").expect("model metadata");

        assert_eq!(info.context_window, Some(1_048_576));
        assert_eq!(info.max_output, Some(384_000));
        assert_eq!(info.supports_tool_call, Some(true));
        assert_eq!(info.supports_reasoning, Some(true));
    }

    #[test]
    fn provider_fields_override_database_limits() {
        let raw = json!({
            "id": "deepseek-v4-pro",
            "context_length": 200000,
            "max_output_tokens": 64000
        });
        let info = model_database_info("deepseek-v4-pro");

        assert_eq!(extract_max_input_tokens(&raw), Some(200_000));
        assert_eq!(extract_max_output_tokens(&raw), Some(64_000));
        assert_eq!(
            extract_max_input_tokens(&raw)
                .or_else(|| info.as_ref().and_then(|item| item.context_window)),
            Some(200_000)
        );
        assert_eq!(
            extract_max_output_tokens(&raw)
                .or_else(|| info.as_ref().and_then(|item| item.max_output)),
            Some(64_000)
        );
    }

    #[test]
    fn remove_model_by_id_only_removes_exact_matches() {
        let mut models = vec![
            json!({"id": "target", "name": "Target"}),
            json!({"id": "target-2", "name": "Other"}),
            json!({"name": "No id"}),
        ];

        assert_eq!(remove_model_by_id(&mut models, "target"), 1);
        assert_eq!(models.len(), 2);
        assert_eq!(
            models[0].get("id").and_then(Value::as_str),
            Some("target-2")
        );
        assert_eq!(models[1].get("name").and_then(Value::as_str), Some("No id"));
    }
}
