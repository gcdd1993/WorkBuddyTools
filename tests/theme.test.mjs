import test from "node:test";
import assert from "node:assert/strict";

import {
  isThemeMode,
  resolveInitialTheme,
  toggleTheme,
} from "../.tmp-tests/theme.js";

test("recognizes only supported theme modes", () => {
  assert.equal(isThemeMode("light"), true);
  assert.equal(isThemeMode("dark"), true);
  assert.equal(isThemeMode("system"), false);
  assert.equal(isThemeMode(""), false);
});

test("prefers stored theme over system preference", () => {
  assert.equal(resolveInitialTheme("light", true), "light");
  assert.equal(resolveInitialTheme("dark", false), "dark");
});

test("falls back to system preference when stored theme is missing or invalid", () => {
  assert.equal(resolveInitialTheme(null, true), "dark");
  assert.equal(resolveInitialTheme(null, false), "light");
  assert.equal(resolveInitialTheme("system", true), "dark");
  assert.equal(resolveInitialTheme("blue", false), "light");
});

test("toggles between light and dark themes", () => {
  assert.equal(toggleTheme("light"), "dark");
  assert.equal(toggleTheme("dark"), "light");
});
