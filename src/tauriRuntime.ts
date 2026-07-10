import { invoke } from "@tauri-apps/api/core";

type TauriGlobal = {
  __TAURI__?: unknown;
  __TAURI_INTERNALS__?: unknown;
};

export function isTauriRuntime(globalObject: unknown = globalThis): boolean {
  const candidate = globalObject as TauriGlobal;
  return Boolean(candidate.__TAURI__ || candidate.__TAURI_INTERNALS__);
}

export function getBrowserPreviewResult(command: string): unknown {
  switch (command) {
    case "get_paths":
      return {
        workbuddyDir: "浏览器预览",
        modelsFile: "Tauri 桌面运行时会读取 WorkBuddy 模型文件",
        providersFile: "Tauri 桌面运行时会读取供应商配置文件",
      };
    case "load_workbuddy_models":
    case "load_providers":
      return [];
    default:
      throw new Error("此操作需要在 Tauri 桌面应用中运行。");
  }
}

export async function invokeCommand<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  if (!isTauriRuntime()) {
    return getBrowserPreviewResult(command) as T;
  }
  return invoke<T>(command, args);
}
