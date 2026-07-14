import { invoke } from "@tauri-apps/api/core";

type TauriGlobal = {
  __TAURI__?: unknown;
  __TAURI_INTERNALS__?: unknown;
};

export function isTauriRuntime(globalObject: unknown = globalThis): boolean {
  const candidate = globalObject as TauriGlobal;
  return Boolean(candidate.__TAURI__ || candidate.__TAURI_INTERNALS__);
}

export function getBrowserPreviewResult(command: string, args?: Record<string, unknown>): unknown {
  switch (command) {
    case "get_paths":
      return {
        workbuddyDir: "浏览器预览",
        modelsFile: "Tauri 桌面运行时会读取 WorkBuddy 模型文件",
        providersFile: "Tauri 桌面运行时会读取供应商配置文件",
      };
    case "load_workbuddy_models":
    case "load_providers":
    case "list_workbuddy_sessions":
      return [];
    case "webdav_fetch_remote_info":
      return null;
    case "load_app_settings":
      return {
        webdav: { baseUrl: "", username: "", password: "", remoteRoot: "WorkBuddySync", passphrase: "" },
      };
    case "save_app_settings":
      return args?.settings;
    case "webdav_test_connection":
    case "webdav_upload_sync":
    case "webdav_download_sync":
    case "webdav_run_sync":
      throw new Error("WebDAV 同步需要在 Tauri 桌面应用中运行。");
    case "update_workbuddy_session":
    case "delete_workbuddy_session":
      throw new Error("会话管理需要在 Tauri 桌面应用中运行。");
    default:
      throw new Error("此操作需要在 Tauri 桌面应用中运行。");
  }
}

export async function invokeCommand<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  if (!isTauriRuntime()) {
    return getBrowserPreviewResult(command, args) as T;
  }
  return invoke<T>(command, args);
}
