import React, { FormEvent, useEffect, useMemo, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import {
  ArrowLeft,
  Check,
  CircleAlert,
  Cloud,
  Database,
  Download,
  Eye,
  EyeOff,
  KeyRound,
  Settings,
  Loader2,
  MessageSquare,
  Moon,
  Pencil,
  Plus,
  RefreshCcw,
  Server,
  Sun,
  Trash2,
  Upload,
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

type TabKey = "models" | "providers" | "sessions";

type WorkBuddySessionSummary = {
  id: string;
  title: string;
  cwd: string;
  status: string;
  model: string;
  createdAt: number | null;
  updatedAt: number | null;
  lastActivityAt: number | null;
  sizeBytes: number;
};

type SessionEditForm = {
  title: string;
  cwd: string;
};

type DeleteSessionResult = {
  message?: string;
};

type SyncStrategy = "smartMerge" | "remoteOverwriteLocal" | "localOverwriteRemote";

type WebDavSyncSettings = {
  baseUrl: string;
  username: string;
  password: string;
  remoteRoot: string;
  passphrase: string;
};

type AppSettings = {
  webdav: WebDavSyncSettings;
};

type WebDavRemoteInfo = {
  generation: string;
  createdAt: string;
  deviceName: string;
  encryptedPackagePath: string;
  encryptedPackageSha256: string;
  encryptedPackageSize: number;
};

type WebDavSyncResult = {
  status: string;
  strategy: SyncStrategy;
  generation?: string | null;
  remotePath?: string | null;
  backupDir?: string | null;
  conflicts: string[];
  message: string;
};

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
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [appSettings, setAppSettings] = useState<AppSettings | null>(null);
  const [settingsDraft, setSettingsDraft] = useState<AppSettings | null>(null);
  const [savingSettings, setSavingSettings] = useState(false);
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
  const [syncSettings, setSyncSettings] = useState<WebDavSyncSettings>({
    baseUrl: "", username: "", password: "", remoteRoot: "WorkBuddySync",
    passphrase: "",
  });
  const [syncStrategy, setSyncStrategy] = useState<SyncStrategy>("smartMerge");
  const [syncLoading, setSyncLoading] = useState("");
  const [remoteInfo, setRemoteInfo] = useState<WebDavRemoteInfo | null>(null);
  const [syncResult, setSyncResult] = useState<WebDavSyncResult | null>(null);
  const [sessions, setSessions] = useState<WorkBuddySessionSummary[]>([]);
  const [sessionsLoading, setSessionsLoading] = useState(false);
  const [deletingSessionId, setDeletingSessionId] = useState("");
  const [editingSession, setEditingSession] = useState<WorkBuddySessionSummary | null>(null);
  const [sessionEditForm, setSessionEditForm] = useState<SessionEditForm>({ title: "", cwd: "" });
  const [savingSession, setSavingSession] = useState(false);
  const lastAutoFetchedProviderId = useRef("");

  useEffect(() => {
    void initializeApp();
  }, []);

  async function initializeApp() {
    try {
      const saved = await invokeCommand<AppSettings>("load_app_settings");
      setAppSettings(saved);
      setSettingsDraft(saved);
      setSyncSettings(saved.webdav);
    } catch (err) {
      setError(toErrorMessage(err));
    }
    await refreshAll();
  }

  function openSettings() {
    if (appSettings) setSettingsDraft(structuredClone(appSettings));
    setSettingsOpen(true);
  }

  function closeSettings() {
    if (appSettings) setSettingsDraft(structuredClone(appSettings));
    setSettingsOpen(false);
  }

  async function saveSettings(event: FormEvent) {
    event.preventDefault();
    if (!settingsDraft) return;
    setSavingSettings(true);
    setError("");
    try {
      const saved = await invokeCommand<AppSettings>("save_app_settings", { settings: settingsDraft });
      setAppSettings(saved);
      setSettingsDraft(structuredClone(saved));
      setSyncSettings(saved.webdav);
      setRemoteInfo(null);
      setSyncResult(null);
      setMessage("设置已保存");
    } catch (err) {
      setError(toErrorMessage(err));
    } finally {
      setSavingSettings(false);
    }
  }

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
    if (activeTab === "sessions" && sessions.length === 0) {
      void refreshSessions();
    }
  }, [activeTab]);

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

  async function refreshSessions() {
    setSessionsLoading(true);
    setError("");
    try {
      setSessions(await invokeCommand<WorkBuddySessionSummary[]>("list_workbuddy_sessions"));
    } catch (err) {
      setError(toErrorMessage(err));
    } finally {
      setSessionsLoading(false);
    }
  }

  async function handleDeleteSession(session: WorkBuddySessionSummary) {
    if (session.status.toLowerCase() === "working") return;
    if (!window.confirm(`确定将会话“${session.title || session.id}”移入回收站吗？`)) return;
    setDeletingSessionId(session.id);
    setError("");
    try {
      const result = await invokeCommand<DeleteSessionResult>("delete_workbuddy_session", { sessionId: session.id });
      setMessage(result.message || "会话已移入回收站");
      await refreshSessions();
    } catch (err) {
      setError(toErrorMessage(err));
    } finally {
      setDeletingSessionId("");
    }
  }

  function openEditSessionDialog(session: WorkBuddySessionSummary) {
    if (session.status.toLowerCase() === "working") {
      setError("正在运行的会话不能编辑，请先结束会话");
      return;
    }
    setEditingSession(session);
    setSessionEditForm({
      title: session.title || "",
      cwd: session.cwd || "",
    });
  }

  function closeEditSessionDialog() {
    if (savingSession) {
      return;
    }
    setEditingSession(null);
    setSessionEditForm({ title: "", cwd: "" });
  }

  async function handleSaveSession(event: FormEvent) {
    event.preventDefault();
    if (!editingSession) return;
    setSavingSession(true);
    setError("");
    setMessage("");
    try {
      await invokeCommand("update_workbuddy_session", {
        input: {
          sessionId: editingSession.id,
          title: sessionEditForm.title,
          cwd: sessionEditForm.cwd,
        },
      });
      setMessage("会话已保存");
      setEditingSession(null);
      setSessionEditForm({ title: "", cwd: "" });
      await refreshSessions();
    } catch (err) {
      setError(toErrorMessage(err));
    } finally {
      setSavingSession(false);
    }
  }

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

  async function runWebDavAction(command: string, strategy?: SyncStrategy) {
    setSyncLoading(command);
    setError("");
    setMessage("");
    try {
      if (command === "webdav_test_connection") {
        await invokeCommand(command, { settings: syncSettings });
        setMessage("WebDAV 连接成功");
      } else if (command === "webdav_fetch_remote_info") {
        const info = await invokeCommand<WebDavRemoteInfo | null>(command, { settings: syncSettings });
        setRemoteInfo(info);
        setMessage(info ? "已读取远端同步信息" : "远端暂无同步包");
      } else {
        const result = await invokeCommand<WebDavSyncResult>(command, {
          settings: syncSettings,
          strategy: strategy ?? syncStrategy,
        });
        setSyncResult(result);
        setMessage(result.message);
        const info = await invokeCommand<WebDavRemoteInfo | null>("webdav_fetch_remote_info", { settings: syncSettings });
        setRemoteInfo(info);
      }
    } catch (err) {
      setError(toErrorMessage(err));
    } finally {
      setSyncLoading("");
    }
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
          <button className="icon-button" type="button" onClick={openSettings} title="应用设置" aria-label="打开应用设置">
            <Settings size={18} />
          </button>
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

      {settingsOpen ? (
        <SettingsPage
          settings={settingsDraft}
          savedSettings={appSettings}
          saving={savingSettings}
          strategy={syncStrategy}
          syncLoading={syncLoading}
          remoteInfo={remoteInfo}
          syncResult={syncResult}
          onChange={setSettingsDraft}
          onCancel={closeSettings}
          onSave={saveSettings}
          onStrategyChange={setSyncStrategy}
          onSyncAction={runWebDavAction}
        />
      ) : <>
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
        <button
          className={activeTab === "sessions" ? "active" : ""}
          type="button"
          role="tab"
          aria-selected={activeTab === "sessions"}
          onClick={() => setActiveTab("sessions")}
        >
          <MessageSquare size={16} />
          <span>会话管理</span>
          <span className="tab-count">{sessions.length}</span>
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
      ) : activeTab === "providers" ? (
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
      ) : (
        <SessionsTab
          sessions={sessions}
          loading={sessionsLoading}
          deletingSessionId={deletingSessionId}
          editingSession={editingSession}
          sessionEditForm={sessionEditForm}
          savingSession={savingSession}
          onRefresh={refreshSessions}
          onEdit={openEditSessionDialog}
          onEditFormChange={setSessionEditForm}
          onSaveEdit={handleSaveSession}
          onCloseEdit={closeEditSessionDialog}
          onDelete={handleDeleteSession}
        />
      )}
      </>}
    </main>
  );
}

function SettingsPage({ settings, savedSettings, saving, strategy, syncLoading, remoteInfo, syncResult, onChange, onCancel, onSave, onStrategyChange, onSyncAction }: {
  settings: AppSettings | null; savedSettings: AppSettings | null; saving: boolean;
  strategy: SyncStrategy; syncLoading: string; remoteInfo: WebDavRemoteInfo | null; syncResult: WebDavSyncResult | null;
  onChange: (settings: AppSettings) => void;
  onCancel: () => void; onSave: (event: FormEvent) => void;
  onStrategyChange: (strategy: SyncStrategy) => void;
  onSyncAction: (command: string, strategy?: SyncStrategy) => void;
}) {
  if (!settings) return <section className="panel settings-page"><EmptyState label="正在读取应用设置..." /></section>;
  const updateWebDav = (field: keyof WebDavSyncSettings, value: string) => onChange({ ...settings, webdav: { ...settings.webdav, [field]: value } });
  return (
    <section className="panel settings-page" aria-labelledby="settings-title">
      <div className="panel-header settings-header"><button className="icon-button" type="button" onClick={onCancel} aria-label="返回主界面" title="返回主界面"><ArrowLeft size={18} /></button><div><h2 id="settings-title">应用设置</h2><p>配置 WebDAV 连接与同步</p></div></div>
      <form className="settings-form" onSubmit={onSave}>
        <div className="settings-section">
          <div className="settings-section-heading"><h3>本地存储</h3><p>本程序设置固定保存到 %USERPROFILE%\.workbuddy\workbuddy-tools\settings.json；WorkBuddy 数据固定读取 %USERPROFILE%\.workbuddy。</p></div>
        </div>
        <div className="settings-section">
          <div className="settings-section-heading"><h3>WebDAV 与同步</h3><p>配置远端连接，并在此备份或恢复会话与模型配置。</p></div>
          <div className="settings-grid">
            <label><span className="field-label">WebDAV 地址</span><input value={settings.webdav.baseUrl} onChange={(e) => updateWebDav("baseUrl", e.target.value)} placeholder="https://dav.jianguoyun.com/dav/" /></label>
            <label><span className="field-label">用户名</span><input value={settings.webdav.username} onChange={(e) => updateWebDav("username", e.target.value)} /></label>
            <label><span className="field-label">密码 / Token</span><input type="password" value={settings.webdav.password} onChange={(e) => updateWebDav("password", e.target.value)} /></label>
            <label><span className="field-label">远端目录</span><input value={settings.webdav.remoteRoot} onChange={(e) => updateWebDav("remoteRoot", e.target.value)} /></label>
            <label><span className="field-label">同步加密密码（选填）</span><input type="password" value={settings.webdav.passphrase} onChange={(e) => updateWebDav("passphrase", e.target.value)} /></label>
          </div>
          <div className="settings-note danger-note"><CircleAlert size={17} /><span>WebDAV 密码和同步加密密码会以明文保存到本机 settings.json，请确保当前 Windows 账户和设置文件安全。</span></div>
          <SyncSettingsSection
            settings={savedSettings?.webdav ?? settings.webdav}
            strategy={strategy}
            loading={syncLoading}
            saving={saving}
            remoteInfo={remoteInfo}
            result={syncResult}
            hasUnsavedChanges={!savedSettings || JSON.stringify(settings) !== JSON.stringify(savedSettings)}
            onCancel={onCancel}
            onStrategyChange={onStrategyChange}
            onAction={onSyncAction}
          />
        </div>
      </form>
    </section>
  );
}

function SessionsTab({
  sessions,
  loading,
  deletingSessionId,
  editingSession,
  sessionEditForm,
  savingSession,
  onRefresh,
  onEdit,
  onEditFormChange,
  onSaveEdit,
  onCloseEdit,
  onDelete,
}: {
  sessions: WorkBuddySessionSummary[];
  loading: boolean;
  deletingSessionId: string;
  editingSession: WorkBuddySessionSummary | null;
  sessionEditForm: SessionEditForm;
  savingSession: boolean;
  onRefresh: () => void;
  onEdit: (session: WorkBuddySessionSummary) => void;
  onEditFormChange: (next: SessionEditForm) => void;
  onSaveEdit: (event: FormEvent) => void;
  onCloseEdit: () => void;
  onDelete: (session: WorkBuddySessionSummary) => void;
}) {
  const [query, setQuery] = useState("");
  const normalizedQuery = query.trim().toLowerCase();
  const visibleSessions = sessions.filter((session) =>
    !normalizedQuery || session.title.toLowerCase().includes(normalizedQuery) || session.cwd.toLowerCase().includes(normalizedQuery),
  );

  return (
    <section className="panel sessions-panel" aria-labelledby="sessions-title">
      <div className="panel-header">
        <div className="session-heading"><h2 id="sessions-title">本机会话管理</h2><p>共 {sessions.length} 个会话，可按标题或工作目录搜索</p></div>
        <div className="session-toolbar">
          <button
            className="icon-button session-refresh-button"
            type="button"
            onClick={onRefresh}
            disabled={loading}
            title="刷新会话列表"
            aria-label="刷新会话列表"
          >
            {loading ? <Loader2 className="spin" size={18} /> : <RefreshCcw size={18} />}
          </button>
          <input
            id="session-search"
            type="search"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="输入标题或工作目录"
            aria-label="搜索会话"
          />
          <span>{visibleSessions.length} / {sessions.length}</span>
        </div>
      </div>
      {visibleSessions.length ? (
        <div className="session-list">
          {visibleSessions.map((session) => {
            const working = session.status.toLowerCase() === "working";
            const deleting = deletingSessionId === session.id;
            return (
              <article className="session-card" key={session.id}>
                <div className="session-card-main">
                  <div className="session-title-row">
                    <h3>{session.title || "未命名会话"}</h3>
                    <span className={`session-status${working ? " working" : ""}`}>{session.status || "unknown"}</span>
                  </div>
                  <p className="session-cwd" title={session.cwd}>{session.cwd || "无工作目录"}</p>
                  <dl className="session-details">
                    <div><dt>模型</dt><dd>{session.model || "未知"}</dd></div>
                    <div><dt>最后活动</dt><dd>{formatSessionTime(session.lastActivityAt || session.updatedAt)}</dd></div>
                    <div><dt>创建时间</dt><dd>{formatSessionTime(session.createdAt)}</dd></div>
                    <div><dt>文件大小</dt><dd>{formatBytes(session.sizeBytes)}</dd></div>
                  </dl>
                </div>
                <div className="session-card-actions">
                  <button className="secondary-button" type="button" disabled={working || Boolean(deletingSessionId)} onClick={() => onEdit(session)} title={working ? "正在运行的会话不能编辑" : "编辑会话"}>
                    <Pencil size={16} />编辑
                  </button>
                  <button className="danger-button" type="button" disabled={working || Boolean(deletingSessionId)} onClick={() => onDelete(session)} title={working ? "正在运行的会话不能删除" : "移入会话回收站"}>
                    {deleting ? <Loader2 className="spin" size={16} /> : <Trash2 size={16} />}{working ? "运行中" : "删除"}
                  </button>
                  {working ? <small>正在运行，无法编辑或删除</small> : null}
                </div>
              </article>
            );
          })}
        </div>
      ) : <EmptyState label={loading ? "正在读取本机会话..." : normalizedQuery ? "没有匹配的会话" : "暂无本机会话"} />}
      {editingSession ? (
        <SessionEditDialog
          session={editingSession}
          form={sessionEditForm}
          saving={savingSession}
          onChange={onEditFormChange}
          onSave={onSaveEdit}
          onClose={onCloseEdit}
        />
      ) : null}
    </section>
  );
}

function SessionEditDialog({
  session,
  form,
  saving,
  onChange,
  onSave,
  onClose,
}: {
  session: WorkBuddySessionSummary;
  form: SessionEditForm;
  saving: boolean;
  onChange: (next: SessionEditForm) => void;
  onSave: (event: FormEvent) => void;
  onClose: () => void;
}) {
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
        className="provider-dialog session-edit-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="session-edit-dialog-title"
      >
        <div className="dialog-header">
          <div>
            <h2 id="session-edit-dialog-title">编辑会话</h2>
            <p>{session.id}</p>
          </div>
          <button
            className="icon-button"
            type="button"
            onClick={onClose}
            disabled={saving}
            aria-label="关闭会话编辑表单"
            title="关闭"
          >
            <X size={18} />
          </button>
        </div>

        <form className="provider-form dialog-form" onSubmit={onSave}>
          <label>
            <span className="field-label">
              会话名称
              <small>必填</small>
            </span>
            <input
              value={form.title}
              onChange={(event) => onChange({ ...form, title: event.target.value })}
              placeholder="输入会话名称"
              autoFocus
              required
            />
          </label>
          <label>
            <span className="field-label">
              工作目录
              <small>必填</small>
            </span>
            <input
              value={form.cwd}
              onChange={(event) => onChange({ ...form, cwd: event.target.value })}
              placeholder={"D:\\OneDrive\\WorkBuddy\\WorkSpace\\Project"}
              required
            />
          </label>

          <div className="dialog-actions">
            <button
              className="secondary-button"
              type="button"
              onClick={onClose}
              disabled={saving}
            >
              取消
            </button>
            <button className="primary-button" type="submit" disabled={saving}>
              {saving ? <Loader2 className="spin" size={16} /> : <Check size={16} />}
              保存修改
            </button>
          </div>
        </form>
      </section>
    </div>
  );
}

function formatSessionTime(value: number | null) {
  if (value === null) return "未知";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "未知";
  return new Intl.DateTimeFormat("zh-CN", { dateStyle: "medium", timeStyle: "short" }).format(date);
}

function SyncSettingsSection({ settings, strategy, loading, saving, remoteInfo, result, hasUnsavedChanges, onCancel, onStrategyChange, onAction }: {
  settings: WebDavSyncSettings;
  strategy: SyncStrategy;
  loading: string;
  saving: boolean;
  remoteInfo: WebDavRemoteInfo | null;
  result: WebDavSyncResult | null;
  hasUnsavedChanges: boolean;
  onCancel: () => void;
  onStrategyChange: (strategy: SyncStrategy) => void;
  onAction: (command: string, strategy?: SyncStrategy) => void;
}) {
  const busy = loading.length > 0;
  const configured = Boolean(settings.baseUrl.trim() && settings.username.trim() && settings.password.trim());
  return (
    <div className="settings-sync" aria-labelledby="sync-title">
      <div className="settings-section-heading"><h3 id="sync-title">同步操作</h3><p>将会话与模型配置打包后同步到 WebDAV。</p></div>
      <div className="sync-content">
        <div className="sync-card">
          {!configured ? <div className="sync-unconfigured"><CircleAlert size={20} /><div><strong>尚未配置 WebDAV</strong><p>请先填写并保存 WebDAV 地址、用户名和密码，再执行同步操作。</p></div></div> : null}
          {hasUnsavedChanges ? <div className="settings-note"><CircleAlert size={17} /><span>设置存在未保存的更改。同步操作只使用已保存的配置，请先保存设置。</span></div> : null}
          <div className="provider-form sync-options">
            <label><span className="field-label">同步策略</span><select value={strategy} onChange={(e) => onStrategyChange(e.target.value as SyncStrategy)}><option value="smartMerge">智能合并</option><option value="remoteOverwriteLocal">远端覆盖本机</option><option value="localOverwriteRemote">本机覆盖远端</option></select></label>
          </div>
          <div className={`warning-box${settings.passphrase.trim() ? "" : " warning-box-danger"}`}>同步包包含会话、models.json 与 model-providers.json（可能包含 API Key）。填写同步加密密码时上传为 workbuddy-sync.zip.enc；留空时将以明文 workbuddy-sync.zip 上传，会话、模型配置及 API Key 将以明文存储在 WebDAV，存在安全风险。</div>
          <div className="sync-actions">
            <button type="button" className="secondary-button" onClick={onCancel} disabled={saving}>取消</button>
            <button type="submit" className="primary-button" disabled={saving}>保存</button>
            <button type="button" className="secondary-button" disabled={busy || !configured || hasUnsavedChanges} onClick={() => onAction("webdav_test_connection")}>{loading === "webdav_test_connection" ? <Loader2 className="spin" size={16} /> : <Cloud size={16} />}测试连接</button>
            <button type="button" className="secondary-button" disabled={busy || !configured || hasUnsavedChanges} onClick={() => onAction("webdav_fetch_remote_info")}><RefreshCcw size={16} />查看远端</button>
            <button type="button" className="primary-button" disabled={busy || !configured || hasUnsavedChanges} onClick={() => onAction("webdav_run_sync")}>{loading === "webdav_run_sync" ? <Loader2 className="spin" size={16} /> : <RefreshCcw size={16} />}执行同步</button>
            <button type="button" className="secondary-button" disabled={busy || !configured || hasUnsavedChanges} onClick={() => onAction("webdav_upload_sync", "localOverwriteRemote")}><Upload size={16} />上传本机覆盖远端</button>
            <button type="button" className="secondary-button" disabled={busy || !configured || hasUnsavedChanges} onClick={() => onAction("webdav_download_sync", "remoteOverwriteLocal")}><Download size={16} />下载远端覆盖本机</button>
          </div>
        </div>
        {remoteInfo ? <div className="result-card"><h3>远端同步包</h3><dl className="detail-list"><dt>代次</dt><dd>{remoteInfo.generation}</dd><dt>创建时间</dt><dd>{remoteInfo.createdAt}</dd><dt>设备</dt><dd>{remoteInfo.deviceName}</dd><dt>大小</dt><dd>{formatBytes(remoteInfo.encryptedPackageSize)}</dd><dt>路径</dt><dd className="mono">{remoteInfo.encryptedPackagePath}</dd><dt>SHA-256</dt><dd className="mono">{remoteInfo.encryptedPackageSha256}</dd></dl></div> : null}
        {result ? <div className="result-card"><h3>最近同步结果</h3><p>{result.message}</p><dl className="detail-list">{result.generation ? <><dt>代次</dt><dd>{result.generation}</dd></> : null}{result.remotePath ? <><dt>远端路径</dt><dd className="mono">{result.remotePath}</dd></> : null}{result.backupDir ? <><dt>本机备份</dt><dd className="mono">{result.backupDir}</dd></> : null}</dl>{result.conflicts.length ? <div className="conflict-list"><strong>冲突文件</strong>{result.conflicts.map((path) => <div className="mono" key={path}>{path}</div>)}</div> : null}</div> : null}
      </div>
    </div>
  );
}

function InfoTooltip({ id, children }: { id: string; children: React.ReactNode }) {
  return (
    <span className="info-tooltip">
      <button className="info-tooltip-trigger" type="button" aria-label="查看说明" aria-describedby={id}>
        <CircleAlert aria-hidden="true" size={15} />
      </button>
      <span className="info-tooltip-content" id={id} role="tooltip">{children}</span>
    </span>
  );
}

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
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
