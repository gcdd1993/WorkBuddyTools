export type ThemeMode = "light" | "dark";

export const THEME_STORAGE_KEY = "workbuddy-theme";

export function isThemeMode(value: unknown): value is ThemeMode {
  return value === "light" || value === "dark";
}

export function resolveInitialTheme(
  storedTheme: unknown,
  systemPrefersDark: boolean,
): ThemeMode {
  if (isThemeMode(storedTheme)) {
    return storedTheme;
  }
  return systemPrefersDark ? "dark" : "light";
}

export function toggleTheme(theme: ThemeMode): ThemeMode {
  return theme === "dark" ? "light" : "dark";
}

export function getSystemPrefersDark(): boolean {
  return window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? false;
}

export function readStoredTheme(): string | null {
  try {
    return window.localStorage.getItem(THEME_STORAGE_KEY);
  } catch {
    return null;
  }
}

export function writeStoredTheme(theme: ThemeMode): void {
  try {
    window.localStorage.setItem(THEME_STORAGE_KEY, theme);
  } catch {
    // Theme persistence is a convenience; the visible theme can still update.
  }
}

export function applyThemeToDocument(theme: ThemeMode): void {
  document.documentElement.dataset.theme = theme;
  document.documentElement.style.colorScheme = theme;
}
