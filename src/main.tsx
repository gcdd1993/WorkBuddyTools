import React, { FormEvent, useEffect, useMemo, useState } from "react";
import { createRoot } from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import {
  Check,
  Database,
  KeyRound,
  Loader2,
  Plus,
  RefreshCcw,
  Server,
  Trash2,
} from "lucide-react";
import "./styles.css";

type TabKey = "models" | "providers";

type WorkBuddyModel = {
  id?: string;
  name?: string;
  vendor?: string;
  url?: string;
  apiKey?: string;
  supportsToolCall?: boolean;
  supportsImages?: boolean;
  supportsReasoning?: boolean;
  useCustomProtocol?: boolean;
  maxInputTokens?: number;
  maxOutputTokens?: number;
  [key: string]: unknown;
};

type Provider = {
  id: string;
  name: string;
  baseUrl: string;
  apiKey: string;
  createdAt?: string;
  updatedAt?: string;
  lastFetchedAt?: string;
};

type ModelCapabilities = {
  supportsToolCall: boolean;
  supportsImages: boolean;
  supportsReasoning: boolean;
  useCustomProtocol: boolean;
};

type ProviderModel = {
  id: string;
  name: string;
  providerId: string;
  providerName: string;
  maxInputTokens?: number;
  maxOutputTokens?: number;
  raw: unknown;
  capabilities: ModelCapabilities;
};

type FetchModelsResult = {
  provider: Provider;
  models: ProviderModel[];
};

type AddModelsResult = {
  models: WorkBuddyModel[];
  added: number;
  updated: number;
};

type AppPaths = {
  workbuddyDir: string;
  modelsFile: string;
  providersFile: string;
};

type ProviderForm = {
  id?: string;
  name: string;
  baseUrl: string;
  apiKey: string;
};

const emptyProviderForm: ProviderForm = {
  name: "",
  baseUrl: "",
  apiKey: "",
};

function App() {
  const [activeTab, setActiveTab] = useState<TabKey>("models");
  const [paths, setPaths] = useState<AppPaths | null>(null);
  const [models, setModels] = useState<WorkBuddyModel[]>([]);
  const [providers, setProviders] = useState<Provider[]>([]);
  const [selectedProviderId, setSelectedProviderId] = useState<string>("");
  const [providerForm, setProviderForm] = useState<ProviderForm>(emptyProviderForm);
  const [fetchedModels, setFetchedModels] = useState<ProviderModel[]>([]);
  const [selectedModelIds, setSelectedModelIds] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(true);
  const [fetchingModels, setFetchingModels] = useState(false);
  const [savingProvider, setSavingProvider] = useState(false);
  const [addingModels, setAddingModels] = useState(false);
  const [deletingModelId, setDeletingModelId] = useState<string>("");
  const [message, setMessage] = useState<string>("");
  const [error, setError] = useState<string>("");

  useEffect(() => {
    void refreshAll();
  }, []);

  useEffect(() => {
    if (!selectedProviderId && providers.length > 0) {
      setSelectedProviderId(providers[0].id);
    }
  }, [providers, selectedProviderId]);

  const selectedProvider = useMemo(
    () => providers.find((provider) => provider.id === selectedProviderId),
    [providers, selectedProviderId],
  );

  const configuredIds = useMemo(() => {
    return new Set(models.map((model) => model.id).filter(Boolean) as string[]);
  }, [models]);

  async function refreshAll() {
    setLoading(true);
    setError("");
    try {
      const [nextPaths, nextModels, nextProviders] = await Promise.all([
        invoke<AppPaths>("get_paths"),
        invoke<WorkBuddyModel[]>("load_workbuddy_models"),
        invoke<Provider[]>("load_providers"),
      ]);

      setPaths(nextPaths);
      setModels(nextModels);
      setProviders(nextProviders);
    } catch (err) {
      setError(toErrorMessage(err));
    } finally {
      setLoading(false);
    }
  }

  async function refreshModelsOnly() {
    setError("");
    try {
      setModels(await invoke<WorkBuddyModel[]>("load_workbuddy_models"));
      setMessage("已重新读取 WorkBuddy 模型配置");
    } catch (err) {
      setError(toErrorMessage(err));
    }
  }

  async function handleSaveProvider(event: FormEvent) {
    event.preventDefault();
    setSavingProvider(true);
    setError("");
    setMessage("");

    try {
      const nextProviders = await invoke<Provider[]>("save_provider", {
        input: providerForm,
      });
      setProviders(nextProviders);
      const saved = findSavedProvider(nextProviders, providerForm);
      setSelectedProviderId(saved?.id ?? nextProviders[0]?.id ?? "");
      setProviderForm(emptyProviderForm);
      setMessage("供应商配置已保存");
    } catch (err) {
      setError(toErrorMessage(err));
    } finally {
      setSavingProvider(false);
    }
  }

  async function handleDeleteProvider(providerId: string) {
    setError("");
    setMessage("");

    try {
      const nextProviders = await invoke<Provider[]>("delete_provider", {
        providerId,
      });
      setProviders(nextProviders);
      if (selectedProviderId === providerId) {
        setSelectedProviderId(nextProviders[0]?.id ?? "");
        setFetchedModels([]);
        setSelectedModelIds(new Set());
      }
      setMessage("供应商已删除");
    } catch (err) {
      setError(toErrorMessage(err));
    }
  }

  async function handleFetchModels() {
    if (!selectedProviderId) {
      setError("请先选择供应商");
      return;
    }

    setFetchingModels(true);
    setError("");
    setMessage("");

    try {
      const result = await invoke<FetchModelsResult>("fetch_provider_models", {
        providerId: selectedProviderId,
      });
      setFetchedModels(result.models);
      setSelectedModelIds(new Set());
      setProviders((current) =>
        current.map((provider) =>
          provider.id === result.provider.id ? result.provider : provider,
        ),
      );
      setMessage(`已拉取 ${result.models.length} 个模型`);
    } catch (err) {
      setError(toErrorMessage(err));
    } finally {
      setFetchingModels(false);
    }
  }

  async function handleAddModels() {
    if (!selectedProviderId) {
      setError("请先选择供应商");
      return;
    }

    const modelIds = Array.from(selectedModelIds);
    if (modelIds.length === 0) {
      setError("请选择至少一个模型");
      return;
    }

    setAddingModels(true);
    setError("");
    setMessage("");

    try {
      const result = await invoke<AddModelsResult>("add_models_to_workbuddy", {
        payload: {
          providerId: selectedProviderId,
          modelIds,
          fetchedModels,
        },
      });
      setModels(result.models);
      setSelectedModelIds(new Set());
      setActiveTab("models");
      setMessage(`已添加 ${result.added} 个模型，更新 ${result.updated} 个模型`);
    } catch (err) {
      setError(toErrorMessage(err));
    } finally {
      setAddingModels(false);
    }
  }

  async function handleDeleteWorkBuddyModel(model: WorkBuddyModel) {
    const modelId = typeof model.id === "string" ? model.id : "";
    if (!modelId) {
      setError("模型 ID 为空，无法删除");
      return;
    }

    const label = typeof model.name === "string" && model.name.length > 0 ? model.name : modelId;
    if (!window.confirm(`确认删除模型 ${label}？`)) {
      return;
    }

    setDeletingModelId(modelId);
    setError("");
    setMessage("");

    try {
      const nextModels = await invoke<WorkBuddyModel[]>("delete_workbuddy_model", {
        modelId,
      });
      setModels(nextModels);
      setMessage(`已删除模型 ${label}`);
    } catch (err) {
      setError(toErrorMessage(err));
    } finally {
      setDeletingModelId("");
    }
  }

  function toggleModelSelection(modelId: string) {
    setSelectedModelIds((current) => {
      const next = new Set(current);
      if (next.has(modelId)) {
        next.delete(modelId);
      } else {
        next.add(modelId);
      }
      return next;
    });
  }

  function selectAllFetchedModels() {
    setSelectedModelIds(new Set(fetchedModels.map((model) => model.id)));
  }

  function clearFetchedModelSelection() {
    setSelectedModelIds(new Set());
  }

  return (
    <main className="app-shell">
      <header className="app-header">
        <div>
          <h1>WorkBuddy 模型配置</h1>
          <p>{paths?.modelsFile ?? "读取 WorkBuddy 配置中..."}</p>
        </div>
        <button className="icon-button" onClick={refreshAll} disabled={loading} title="刷新">
          {loading ? <Loader2 className="spin" size={18} /> : <RefreshCcw size={18} />}
        </button>
      </header>

      <nav className="tabs" aria-label="配置视图">
        <button
          className={activeTab === "models" ? "active" : ""}
          onClick={() => setActiveTab("models")}
        >
          <Database size={16} />
          模型列表
        </button>
        <button
          className={activeTab === "providers" ? "active" : ""}
          onClick={() => setActiveTab("providers")}
        >
          <Server size={16} />
          供应商
        </button>
      </nav>

      {error ? <div className="notice error">{error}</div> : null}
      {message ? <div className="notice success">{message}</div> : null}

      {activeTab === "models" ? (
        <ModelsTab
          models={models}
          onRefresh={refreshModelsOnly}
          onDeleteModel={handleDeleteWorkBuddyModel}
          loading={loading}
          deletingModelId={deletingModelId}
        />
      ) : (
        <ProvidersTab
          providers={providers}
          selectedProvider={selectedProvider}
          selectedProviderId={selectedProviderId}
          providerForm={providerForm}
          fetchedModels={fetchedModels}
          selectedModelIds={selectedModelIds}
          configuredIds={configuredIds}
          fetchingModels={fetchingModels}
          savingProvider={savingProvider}
          addingModels={addingModels}
          paths={paths}
          onProviderSelect={setSelectedProviderId}
          onProviderFormChange={setProviderForm}
          onProviderEdit={(provider) =>
            setProviderForm({
              id: provider.id,
              name: provider.name,
              baseUrl: provider.baseUrl,
              apiKey: provider.apiKey,
            })
          }
          onProviderDelete={handleDeleteProvider}
          onProviderSave={handleSaveProvider}
          onFetchModels={handleFetchModels}
          onToggleModel={toggleModelSelection}
          onSelectAll={selectAllFetchedModels}
          onClearSelection={clearFetchedModelSelection}
          onAddModels={handleAddModels}
        />
      )}
    </main>
  );
}

function ModelsTab({
  models,
  loading,
  deletingModelId,
  onRefresh,
  onDeleteModel,
}: {
  models: WorkBuddyModel[];
  loading: boolean;
  deletingModelId: string;
  onRefresh: () => void;
  onDeleteModel: (model: WorkBuddyModel) => void;
}) {
  return (
    <section className="panel">
      <div className="panel-header">
        <div>
          <h2>已配置模型</h2>
          <p>{models.length} 个 WorkBuddy 模型</p>
        </div>
        <button className="secondary-button" onClick={onRefresh} disabled={loading}>
          <RefreshCcw size={16} />
          重新读取
        </button>
      </div>

      <div className="table-wrap">
        <table>
          <thead>
            <tr>
              <th>ID</th>
              <th>名称</th>
              <th>Token 上限</th>
              <th>能力</th>
              <th className="action-column">操作</th>
            </tr>
          </thead>
          <tbody>
            {models.map((model, index) => (
              <tr key={`${model.id ?? "unknown"}-${index}`}>
                <td className="mono">{stringValue(model.id)}</td>
                <td>{stringValue(model.name)}</td>
                <td className="action-cell">
                  <TokenLimits
                    maxInputTokens={numberValue(model.maxInputTokens)}
                    maxOutputTokens={numberValue(model.maxOutputTokens)}
                  />
                </td>
                <td>
                  <CapabilityBadges
                    capabilities={{
                      supportsToolCall: Boolean(model.supportsToolCall),
                      supportsImages: Boolean(model.supportsImages),
                      supportsReasoning: Boolean(model.supportsReasoning),
                      useCustomProtocol: Boolean(model.useCustomProtocol),
                    }}
                  />
                </td>
                <td>
                  <button
                    className="icon-button danger"
                    type="button"
                    title="删除模型"
                    disabled={
                      !model.id ||
                      deletingModelId === model.id ||
                      loading
                    }
                    onClick={() => onDeleteModel(model)}
                  >
                    {deletingModelId === model.id ? (
                      <Loader2 className="spin" size={16} />
                    ) : (
                      <Trash2 size={16} />
                    )}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {models.length === 0 ? <EmptyState label="WorkBuddy 模型配置为空" /> : null}
    </section>
  );
}

function ProvidersTab({
  providers,
  selectedProvider,
  selectedProviderId,
  providerForm,
  fetchedModels,
  selectedModelIds,
  configuredIds,
  fetchingModels,
  savingProvider,
  addingModels,
  paths,
  onProviderSelect,
  onProviderFormChange,
  onProviderEdit,
  onProviderDelete,
  onProviderSave,
  onFetchModels,
  onToggleModel,
  onSelectAll,
  onClearSelection,
  onAddModels,
}: {
  providers: Provider[];
  selectedProvider?: Provider;
  selectedProviderId: string;
  providerForm: ProviderForm;
  fetchedModels: ProviderModel[];
  selectedModelIds: Set<string>;
  configuredIds: Set<string>;
  fetchingModels: boolean;
  savingProvider: boolean;
  addingModels: boolean;
  paths: AppPaths | null;
  onProviderSelect: (providerId: string) => void;
  onProviderFormChange: (next: ProviderForm) => void;
  onProviderEdit: (provider: Provider) => void;
  onProviderDelete: (providerId: string) => void;
  onProviderSave: (event: FormEvent) => void;
  onFetchModels: () => void;
  onToggleModel: (modelId: string) => void;
  onSelectAll: () => void;
  onClearSelection: () => void;
  onAddModels: () => void;
}) {
  return (
    <section className="providers-grid">
      <div className="panel">
        <div className="panel-header">
          <div>
            <h2>供应商配置</h2>
            <p>{paths?.providersFile ?? "model-providers.json"}</p>
          </div>
        </div>

        <form className="provider-form" onSubmit={onProviderSave}>
          <label>
            <span>供应商名称</span>
            <input
              value={providerForm.name}
              onChange={(event) =>
                onProviderFormChange({ ...providerForm, name: event.target.value })
              }
              placeholder="DeepSeek"
              autoComplete="off"
            />
          </label>
          <label>
            <span>API 请求地址</span>
            <input
              value={providerForm.baseUrl}
              onChange={(event) =>
                onProviderFormChange({ ...providerForm, baseUrl: event.target.value })
              }
              placeholder="https://api.example.com/v1"
              autoComplete="off"
            />
          </label>
          <label>
            <span>API Key</span>
            <div className="key-input">
              <KeyRound size={16} />
              <input
                value={providerForm.apiKey}
                onChange={(event) =>
                  onProviderFormChange({ ...providerForm, apiKey: event.target.value })
                }
                placeholder="sk-..."
                type="password"
                autoComplete="off"
              />
            </div>
          </label>

          <div className="form-actions">
            <button className="primary-button" type="submit" disabled={savingProvider}>
              {savingProvider ? <Loader2 className="spin" size={16} /> : <Plus size={16} />}
              {providerForm.id ? "保存修改" : "添加供应商"}
            </button>
            {providerForm.id ? (
              <button
                className="secondary-button"
                type="button"
                onClick={() => onProviderFormChange(emptyProviderForm)}
              >
                取消编辑
              </button>
            ) : null}
          </div>
        </form>

        <div className="provider-list">
          {providers.map((provider) => (
            <div
              className={`provider-row ${provider.id === selectedProviderId ? "selected" : ""}`}
              key={provider.id}
            >
              <button
                className="provider-main"
                type="button"
                onClick={() => onProviderSelect(provider.id)}
              >
                <strong>{provider.name}</strong>
                <span>{provider.baseUrl}</span>
              </button>
              <button
                className="icon-button"
                type="button"
                onClick={() => onProviderEdit(provider)}
                title="编辑供应商"
              >
                <Check size={16} />
              </button>
              <button
                className="icon-button danger"
                type="button"
                onClick={() => onProviderDelete(provider.id)}
                title="删除供应商"
              >
                <Trash2 size={16} />
              </button>
            </div>
          ))}
        </div>

        {providers.length === 0 ? <EmptyState label="还没有供应商配置" /> : null}
      </div>

      <div className="panel">
        <div className="panel-header">
          <div>
            <h2>供应商模型</h2>
            <p>{selectedProvider ? selectedProvider.name : "请选择供应商"}</p>
          </div>
          <button
            className="secondary-button"
            onClick={onFetchModels}
            disabled={!selectedProvider || fetchingModels}
          >
            {fetchingModels ? <Loader2 className="spin" size={16} /> : <RefreshCcw size={16} />}
            拉取模型
          </button>
        </div>

        <div className="selection-toolbar">
          <span>{selectedModelIds.size} / {fetchedModels.length} 已选</span>
          <div>
            <button className="text-button" type="button" onClick={onSelectAll}>
              全选
            </button>
            <button className="text-button" type="button" onClick={onClearSelection}>
              清空
            </button>
          </div>
        </div>

        <div className="provider-model-list">
          {fetchedModels.map((model) => {
            const selected = selectedModelIds.has(model.id);
            const configured = configuredIds.has(model.id);
            return (
              <button
                className={`model-choice ${selected ? "selected" : ""}`}
                key={model.id}
                type="button"
                onClick={() => onToggleModel(model.id)}
              >
                <span className="checkbox">{selected ? <Check size={14} /> : null}</span>
                <span className="model-choice-main">
                  <span className="mono">{model.id}</span>
                  <TokenLimits
                    maxInputTokens={model.maxInputTokens}
                    maxOutputTokens={model.maxOutputTokens}
                    compact
                  />
                  <CapabilityBadges capabilities={model.capabilities} compact />
                </span>
                {configured ? <span className="configured-pill">已在 WorkBuddy</span> : null}
              </button>
            );
          })}
        </div>

        {fetchedModels.length === 0 ? <EmptyState label="拉取后会在这里显示模型" /> : null}

        <div className="footer-actions">
          <button
            className="primary-button"
            type="button"
            onClick={onAddModels}
            disabled={selectedModelIds.size === 0 || addingModels}
          >
            {addingModels ? <Loader2 className="spin" size={16} /> : <Plus size={16} />}
            添加到 WorkBuddy
          </button>
        </div>
      </div>
    </section>
  );
}

function CapabilityBadges({
  capabilities,
  compact = false,
}: {
  capabilities: ModelCapabilities;
  compact?: boolean;
}) {
  const items = [
    ["工具", capabilities.supportsToolCall],
    ["图片", capabilities.supportsImages],
    ["推理", capabilities.supportsReasoning],
    ["自定义协议", capabilities.useCustomProtocol],
  ] as const;

  return (
    <div className={`badges ${compact ? "compact" : ""}`}>
      {items.map(([label, enabled]) => (
        <span className={enabled ? "badge enabled" : "badge"} key={label}>
          {label}
        </span>
      ))}
    </div>
  );
}

function TokenLimits({
  maxInputTokens,
  maxOutputTokens,
  compact = false,
}: {
  maxInputTokens?: number;
  maxOutputTokens?: number;
  compact?: boolean;
}) {
  return (
    <div className={`token-limits ${compact ? "compact" : ""}`}>
      <span>输入 {formatTokenLimit(maxInputTokens)}</span>
      <span>输出 {formatTokenLimit(maxOutputTokens)}</span>
    </div>
  );
}

function EmptyState({ label }: { label: string }) {
  return <div className="empty-state">{label}</div>;
}

function stringValue(value: unknown) {
  if (typeof value === "string" && value.length > 0) {
    return value;
  }
  return "-";
}

function numberValue(value: unknown) {
  if (typeof value === "number" && Number.isFinite(value)) {
    return value;
  }
  if (typeof value === "string" && value.trim().length > 0) {
    const parsed = Number(value);
    return Number.isFinite(parsed) ? parsed : undefined;
  }
  return undefined;
}

function formatTokenLimit(value?: number) {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    return "-";
  }
  return new Intl.NumberFormat("zh-CN").format(value);
}

function toErrorMessage(err: unknown) {
  if (err instanceof Error) {
    return err.message;
  }
  if (typeof err === "string") {
    return err;
  }
  return "操作失败";
}

function findSavedProvider(providers: Provider[], form: ProviderForm) {
  if (form.id) {
    return providers.find((provider) => provider.id === form.id);
  }
  return providers.find((provider) => provider.name === form.name);
}

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
