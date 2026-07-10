import React, { FormEvent, useEffect, useMemo, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import {
  Check,
  Database,
  Eye,
  EyeOff,
  KeyRound,
  Loader2,
  Moon,
  Pencil,
  Plus,
  RefreshCcw,
  Server,
  Sun,
  Trash2,
  X,
} from "lucide-react";
import {
  applyThemeToDocument,
  getSystemPrefersDark,
  readStoredTheme,
  resolveInitialTheme,
  ThemeMode,
  toggleTheme,
  writeStoredTheme,
} from "./theme";
import { invokeCommand } from "./tauriRuntime";
import {
  buildProviderForm,
  ProviderFormValue,
  shouldAutoFetchProviderModels,
} from "./providerWorkflow";
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

type ProviderForm = ProviderFormValue;

function App() {
  const [activeTab, setActiveTab] = useState<TabKey>("models");
  const [theme, setTheme] = useState<ThemeMode>(() =>
    resolveAppTheme(),
  );
  const [paths, setPaths] = useState<AppPaths | null>(null);
  const [models, setModels] = useState<WorkBuddyModel[]>([]);
  const [providers, setProviders] = useState<Provider[]>([]);
  const [selectedProviderId, setSelectedProviderId] = useState<string>("");
  const [providerForm, setProviderForm] = useState<ProviderForm>(() => buildProviderForm());
  const [isProviderDialogOpen, setIsProviderDialogOpen] = useState(false);
  const [fetchedModels, setFetchedModels] = useState<ProviderModel[]>([]);
  const [selectedModelIds, setSelectedModelIds] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(true);
  const [fetchingModels, setFetchingModels] = useState(false);
  const [savingProvider, setSavingProvider] = useState(false);
  const [addingModels, setAddingModels] = useState(false);
  const [deletingModelId, setDeletingModelId] = useState<string>("");
  const [message, setMessage] = useState<string>("");
  const [error, setError] = useState<string>("");
  const lastAutoFetchedProviderId = useRef("");

  useEffect(() => {
    void refreshAll();
  }, []);

  useEffect(() => {
    applyThemeToDocument(theme);
    writeStoredTheme(theme);
  }, [theme]);

  useEffect(() => {
    if (!message) {
      return;
    }

    const timeoutId = window.setTimeout(() => setMessage(""), 4000);
    return () => window.clearTimeout(timeoutId);
  }, [message]);

  useEffect(() => {
    if (!error) {
      return;
    }

    const timeoutId = window.setTimeout(() => setError(""), 6000);
    return () => window.clearTimeout(timeoutId);
  }, [error]);

  useEffect(() => {
    if (!selectedProviderId && providers.length > 0) {
      setSelectedProviderId(providers[0].id);
    }
  }, [providers, selectedProviderId]);

  useEffect(() => {
    if (
      shouldAutoFetchProviderModels({
        activeTab,
        selectedProviderId,
        lastFetchedProviderId: lastAutoFetchedProviderId.current,
      })
    ) {
      void fetchModelsForProvider(selectedProviderId, { announce: false });
    }
  }, [activeTab, selectedProviderId]);

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
        invokeCommand<AppPaths>("get_paths"),
        invokeCommand<WorkBuddyModel[]>("load_workbuddy_models"),
        invokeCommand<Provider[]>("load_providers"),
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
      setModels(await invokeCommand<WorkBuddyModel[]>("load_workbuddy_models"));
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
      const nextProviders = await invokeCommand<Provider[]>("save_provider", {
        input: providerForm,
      });
      setProviders(nextProviders);
      const saved = findSavedProvider(nextProviders, providerForm);
      const savedProviderId = saved?.id ?? nextProviders[0]?.id ?? "";
      const selectedBeforeSave = selectedProviderId;
      setSelectedProviderId(savedProviderId);
      setProviderForm(buildProviderForm());
      setIsProviderDialogOpen(false);
      if (
        activeTab === "providers" &&
        savedProviderId.length > 0 &&
        savedProviderId === selectedBeforeSave
      ) {
        await fetchModelsForProvider(savedProviderId, { announce: false });
      }
      setMessage("供应商配置已保存");
    } catch (err) {
      setError(toErrorMessage(err));
    } finally {
      setSavingProvider(false);
    }
  }

  async function handleDeleteProvider(providerId: string) {
    const provider = providers.find((item) => item.id === providerId);
    const label = provider?.name ?? providerId;
    if (!window.confirm(`确认删除供应商 ${label}？`)) {
      return;
    }

    setError("");
    setMessage("");

    try {
      const nextProviders = await invokeCommand<Provider[]>("delete_provider", {
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

  async function fetchModelsForProvider(
    providerId: string,
    { announce = true }: { announce?: boolean } = {},
  ) {
    if (!providerId) {
      setError("请先选择供应商");
      return;
    }

    setFetchingModels(true);
    setError("");
    if (announce) {
      setMessage("");
    }

    try {
      const result = await invokeCommand<FetchModelsResult>("fetch_provider_models", {
        providerId,
      });
      setFetchedModels(result.models);
      setSelectedModelIds(new Set());
      lastAutoFetchedProviderId.current = providerId;
      setProviders((current) =>
        current.map((provider) =>
          provider.id === result.provider.id ? result.provider : provider,
        ),
      );
      if (announce) {
        setMessage(`已拉取 ${result.models.length} 个模型`);
      }
    } catch (err) {
      setError(toErrorMessage(err));
    } finally {
      setFetchingModels(false);
    }
  }

  async function handleFetchModels() {
    await fetchModelsForProvider(selectedProviderId);
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
      const result = await invokeCommand<AddModelsResult>("add_models_to_workbuddy", {
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
      const nextModels = await invokeCommand<WorkBuddyModel[]>("delete_workbuddy_model", {
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

  function openCreateProviderDialog() {
    setProviderForm(buildProviderForm());
    setIsProviderDialogOpen(true);
  }

  function openEditProviderDialog(provider: Provider) {
    setProviderForm(buildProviderForm(provider));
    setIsProviderDialogOpen(true);
  }

  function closeProviderDialog() {
    if (savingProvider) {
      return;
    }
    setIsProviderDialogOpen(false);
    setProviderForm(buildProviderForm());
  }

  function handleProviderSelect(providerId: string) {
    if (providerId === selectedProviderId) {
      return;
    }
    setSelectedProviderId(providerId);
    setFetchedModels([]);
    setSelectedModelIds(new Set());
  }

  return (
    <main className="app-shell">
      <div className="toast-region" aria-live="polite" aria-atomic="true">
        {error ? <div className="toast error" role="alert">{error}</div> : null}
        {message ? <div className="toast success" role="status">{message}</div> : null}
      </div>

      <header className="app-header">
        <div className="title-block">
          <span className="eyebrow">WorkBuddy Tools</span>
          <h1>WorkBuddy 模型配置</h1>
          <p>{paths?.modelsFile ?? "读取 WorkBuddy 配置中..."}</p>
        </div>
        <div className="header-actions">
          <div className="summary-pill" aria-label={`已配置 ${models.length} 个模型`}>
            <Database size={16} />
            <span>{models.length}</span>
            <small>模型</small>
          </div>
          <div className="summary-pill" aria-label={`已配置 ${providers.length} 个供应商`}>
            <Server size={16} />
            <span>{providers.length}</span>
            <small>供应商</small>
          </div>
          <button
            className="icon-button theme-toggle"
            type="button"
            onClick={() => setTheme((current) => toggleTheme(current))}
            title={theme === "dark" ? "切换到白天皮肤" : "切换到黑夜皮肤"}
            aria-label={theme === "dark" ? "切换到白天皮肤" : "切换到黑夜皮肤"}
          >
            {theme === "dark" ? <Sun size={18} /> : <Moon size={18} />}
          </button>
          <button
            className="icon-button"
            type="button"
            onClick={refreshAll}
            disabled={loading}
            title="刷新全部配置"
            aria-label="刷新全部配置"
          >
            {loading ? <Loader2 className="spin" size={18} /> : <RefreshCcw size={18} />}
          </button>
        </div>
      </header>

      <nav className="tabs" aria-label="配置视图" role="tablist">
        <button
          className={activeTab === "models" ? "active" : ""}
          type="button"
          role="tab"
          aria-selected={activeTab === "models"}
          onClick={() => setActiveTab("models")}
        >
          <Database size={16} />
          <span>模型列表</span>
          <span className="tab-count">{models.length}</span>
        </button>
        <button
          className={activeTab === "providers" ? "active" : ""}
          type="button"
          role="tab"
          aria-selected={activeTab === "providers"}
          onClick={() => setActiveTab("providers")}
        >
          <Server size={16} />
          <span>供应商</span>
          <span className="tab-count">{providers.length}</span>
        </button>
      </nav>

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
          isProviderDialogOpen={isProviderDialogOpen}
          fetchedModels={fetchedModels}
          selectedModelIds={selectedModelIds}
          configuredIds={configuredIds}
          fetchingModels={fetchingModels}
          savingProvider={savingProvider}
          addingModels={addingModels}
          paths={paths}
          onProviderCreate={openCreateProviderDialog}
          onProviderSelect={handleProviderSelect}
          onProviderFormChange={setProviderForm}
          onProviderEdit={openEditProviderDialog}
          onProviderDialogClose={closeProviderDialog}
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
    <section className="panel" aria-labelledby="models-title">
      <div className="panel-header">
        <div>
          <h2 id="models-title">已配置模型</h2>
          <p>{models.length} 个 WorkBuddy 模型</p>
        </div>
        <button
          className="secondary-button"
          type="button"
          onClick={onRefresh}
          disabled={loading}
        >
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
                <td className="token-cell">
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
                    aria-label={`删除模型 ${stringValue(model.name)}`}
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
  isProviderDialogOpen,
  fetchedModels,
  selectedModelIds,
  configuredIds,
  fetchingModels,
  savingProvider,
  addingModels,
  paths,
  onProviderCreate,
  onProviderSelect,
  onProviderFormChange,
  onProviderEdit,
  onProviderDialogClose,
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
  isProviderDialogOpen: boolean;
  fetchedModels: ProviderModel[];
  selectedModelIds: Set<string>;
  configuredIds: Set<string>;
  fetchingModels: boolean;
  savingProvider: boolean;
  addingModels: boolean;
  paths: AppPaths | null;
  onProviderCreate: () => void;
  onProviderSelect: (providerId: string) => void;
  onProviderFormChange: (next: ProviderForm) => void;
  onProviderEdit: (provider: Provider) => void;
  onProviderDialogClose: () => void;
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
      <div className="panel provider-list-panel" aria-labelledby="providers-title">
        <div className="panel-header">
          <div>
            <h2 id="providers-title">供应商列表</h2>
            <p>{paths?.providersFile ?? "model-providers.json"}</p>
          </div>
          <button className="primary-button" type="button" onClick={onProviderCreate}>
            <Plus size={16} />
            添加供应商
          </button>
        </div>

        <div className="provider-list">
          {providers.map((provider) => (
            <div
              className={`provider-row ${provider.id === selectedProviderId ? "selected" : ""}`}
              key={provider.id}
            >
              <button
                className="provider-main"
                type="button"
                aria-pressed={provider.id === selectedProviderId}
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
                aria-label={`编辑供应商 ${provider.name}`}
              >
                <Pencil size={16} />
              </button>
              <button
                className="icon-button danger"
                type="button"
                onClick={() => onProviderDelete(provider.id)}
                title="删除供应商"
                aria-label={`删除供应商 ${provider.name}`}
              >
                <Trash2 size={16} />
              </button>
            </div>
          ))}
          {providers.length === 0 ? <EmptyState label="还没有供应商配置" /> : null}
        </div>
      </div>

      <div className="panel" aria-labelledby="provider-models-title">
        <div className="panel-header">
          <div>
            <h2 id="provider-models-title">供应商模型</h2>
            <p>{selectedProvider ? selectedProvider.name : "请选择供应商"}</p>
          </div>
          <button
            className="secondary-button"
            type="button"
            onClick={onFetchModels}
            disabled={!selectedProvider || fetchingModels}
          >
            {fetchingModels ? <Loader2 className="spin" size={16} /> : <RefreshCcw size={16} />}
            重新拉取
          </button>
        </div>

        <div className="selection-toolbar">
          <span>{selectedModelIds.size} / {fetchedModels.length} 已选</span>
          <div>
            <button
              className="text-button"
              type="button"
              onClick={onSelectAll}
              disabled={fetchedModels.length === 0}
            >
              全选
            </button>
            <button
              className="text-button"
              type="button"
              onClick={onClearSelection}
              disabled={selectedModelIds.size === 0}
            >
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
                aria-pressed={selected}
                onClick={() => onToggleModel(model.id)}
              >
                <span className="checkbox">{selected ? <Check size={14} /> : null}</span>
                <span className="model-choice-main">
                  <span className="model-choice-title-row">
                    <span className="mono">{model.id}</span>
                    {configured ? <span className="configured-pill">已在 WorkBuddy</span> : null}
                  </span>
                  <span className="model-choice-meta">
                    <TokenLimits
                      maxInputTokens={model.maxInputTokens}
                      maxOutputTokens={model.maxOutputTokens}
                      compact
                    />
                    <CapabilityBadges capabilities={model.capabilities} compact />
                  </span>
                </span>
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

      {isProviderDialogOpen ? (
        <ProviderDialog
          providerForm={providerForm}
          savingProvider={savingProvider}
          onProviderFormChange={onProviderFormChange}
          onProviderSave={onProviderSave}
          onClose={onProviderDialogClose}
        />
      ) : null}
    </section>
  );
}

function ProviderDialog({
  providerForm,
  savingProvider,
  onProviderFormChange,
  onProviderSave,
  onClose,
}: {
  providerForm: ProviderForm;
  savingProvider: boolean;
  onProviderFormChange: (next: ProviderForm) => void;
  onProviderSave: (event: FormEvent) => void;
  onClose: () => void;
}) {
  const [showApiKey, setShowApiKey] = useState(false);
  const title = providerForm.id ? "编辑供应商" : "添加供应商";

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        onClose();
      }
    }

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  return (
    <div className="modal-backdrop" role="presentation">
      <section
        className="provider-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="provider-dialog-title"
      >
        <div className="dialog-header">
          <div>
            <h2 id="provider-dialog-title">{title}</h2>
            <p>保存后会回到供应商列表</p>
          </div>
          <button
            className="icon-button"
            type="button"
            onClick={onClose}
            disabled={savingProvider}
            aria-label="关闭供应商表单"
            title="关闭"
          >
            <X size={18} />
          </button>
        </div>

        <form className="provider-form dialog-form" onSubmit={onProviderSave}>
          <label>
            <span className="field-label">
              供应商名称
              <small>必填</small>
            </span>
            <input
              value={providerForm.name}
              onChange={(event) =>
                onProviderFormChange({ ...providerForm, name: event.target.value })
              }
              placeholder="DeepSeek"
              autoComplete="organization"
              autoFocus
              required
            />
          </label>
          <label>
            <span className="field-label">
              API 请求地址
              <small>必填</small>
            </span>
            <input
              value={providerForm.baseUrl}
              onChange={(event) =>
                onProviderFormChange({ ...providerForm, baseUrl: event.target.value })
              }
              placeholder="https://api.example.com/v1"
              type="url"
              autoComplete="url"
              required
            />
          </label>
          <label>
            <span className="field-label">
              API Key
              <small>必填</small>
            </span>
            <div className="key-input">
              <KeyRound size={16} aria-hidden="true" />
              <input
                value={providerForm.apiKey}
                onChange={(event) =>
                  onProviderFormChange({ ...providerForm, apiKey: event.target.value })
                }
                placeholder="sk-..."
                type={showApiKey ? "text" : "password"}
                autoComplete="new-password"
                required
              />
              <button
                className="input-icon-button"
                type="button"
                onClick={() => setShowApiKey((current) => !current)}
                aria-label={showApiKey ? "隐藏 API Key" : "显示 API Key"}
                title={showApiKey ? "隐藏 API Key" : "显示 API Key"}
              >
                {showApiKey ? <EyeOff size={16} /> : <Eye size={16} />}
              </button>
            </div>
          </label>

          <div className="dialog-actions">
            <button
              className="secondary-button"
              type="button"
              onClick={onClose}
              disabled={savingProvider}
            >
              取消
            </button>
            <button className="primary-button" type="submit" disabled={savingProvider}>
              {savingProvider ? <Loader2 className="spin" size={16} /> : <Plus size={16} />}
              {providerForm.id ? "保存修改" : "添加供应商"}
            </button>
          </div>
        </form>
      </section>
    </div>
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

  const className = `badges ${compact ? "compact" : ""}`;
  const content = items.map(([label, enabled]) => (
    <span className={enabled ? "badge enabled" : "badge"} key={label}>
      {label}
    </span>
  ));

  if (compact) {
    return <span className={className}>{content}</span>;
  }

  return (
    <div className={className}>
      {content}
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
  const className = `token-limits ${compact ? "compact" : ""}`;
  const content = (
    <>
      <span>输入 {formatTokenLimit(maxInputTokens)}</span>
      <span>输出 {formatTokenLimit(maxOutputTokens)}</span>
    </>
  );

  if (compact) {
    return <span className={className}>{content}</span>;
  }

  return (
    <div className={className}>
      {content}
    </div>
  );
}

function EmptyState({ label }: { label: string }) {
  return <div className="empty-state">{label}</div>;
}

function resolveAppTheme() {
  return resolveInitialTheme(readStoredTheme(), getSystemPrefersDark());
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

applyThemeToDocument(resolveAppTheme());

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
