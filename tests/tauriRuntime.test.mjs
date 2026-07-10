import test from "node:test";
import assert from "node:assert/strict";

import {
  getBrowserPreviewResult,
  isTauriRuntime,
} from "../.tmp-tests/tauriRuntime.js";

test("detects Tauri globals without depending on the real window", () => {
  assert.equal(isTauriRuntime({}), false);
  assert.equal(isTauriRuntime({ __TAURI_INTERNALS__: {} }), true);
  assert.equal(isTauriRuntime({ __TAURI__: {} }), true);
});

test("returns empty browser preview data for read commands", () => {
  assert.deepEqual(getBrowserPreviewResult("load_workbuddy_models"), []);
  assert.deepEqual(getBrowserPreviewResult("load_providers"), []);

  assert.deepEqual(getBrowserPreviewResult("get_paths"), {
    workbuddyDir: "浏览器预览",
    modelsFile: "Tauri 桌面运行时会读取 WorkBuddy 模型文件",
    providersFile: "Tauri 桌面运行时会读取供应商配置文件",
  });
});

test("blocks mutating commands outside the Tauri runtime", () => {
  assert.throws(
    () => getBrowserPreviewResult("save_provider"),
    /Tauri 桌面应用/,
  );
});
