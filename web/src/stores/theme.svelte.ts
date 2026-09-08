export type Theme = "light" | "dark" | "system";

const key = "gofro-theme";

export const appearance = $state<{ theme: Theme }>({ theme: "system" });

function isTheme(value: string | null): value is Theme {
  return value === "light" || value === "dark" || value === "system";
}

export function setTheme(theme: Theme): void {
  appearance.theme = theme;
  document.documentElement.dataset.theme = theme;
  try { localStorage.setItem(key, theme); } catch { /* Storage can be unavailable. */ }
}

export function initializeTheme(): void {
  let stored: string | null = null;
  try { stored = localStorage.getItem(key); } catch { /* Storage can be unavailable. */ }
  appearance.theme = isTheme(stored) ? stored : "system";
  document.documentElement.dataset.theme = appearance.theme;
}
