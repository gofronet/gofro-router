<script lang="ts">
  import RouterIcon from "lucide-svelte/icons/router";
  import ChevronRight from "lucide-svelte/icons/chevron-right";
  import RefreshCw from "lucide-svelte/icons/refresh-cw";
  import { getAppContext } from "../app-context";
  import { formatDuration } from "../format";
  import { p } from "../router";
  import { appearance, setTheme, type Theme } from "../stores/theme.svelte";
  import Dialog from "../components/dialog.svelte";
  const app = getAppContext();
  const status = $derived(app.status);
  const themes: { value: Theme; label: string }[] = [{ value: "light", label: "Светлая" }, { value: "dark", label: "Тёмная" }, { value: "system", label: "Системная" }];
  let confirm = $state<"update" | "reboot" | null>(null);
  let restarting = $state(false);
  const updateMessage = $derived(status.update.running ? "Загружаем и устанавливаем обновление. Панель может ненадолго отключиться." : status.update.result === "updated" ? `Установлена версия ${status.version}.` : status.update.result === "current" ? "Установлена последняя версия." : status.update.result === "failed" ? "Не удалось обновить. Проверьте интернет и попробуйте снова." : "");

  async function perform() {
    if (app.busy) return;
    if (confirm === "reboot") {
      if (await app.rebootRouter()) { confirm = null; restarting = true; }
    } else {
      await app.startUpdate();
      if (!app.actionError) confirm = null;
    }
  }
</script>

<svelte:head><title>Система · GofroRouter</title></svelte:head>
{#if restarting}
  <section class="panel p-6" role="status"><h2>Роутер перезагружается</h2><p class="support-text">Интернет и Wi-Fi временно отключатся. Дождитесь своей сети и подключитесь заново. После перезагрузки потребуется войти в панель.</p><button class="btn primary mt-5" type="button" onclick={app.initializeAuth}>Проверить подключение</button></section>
{:else}
  <div class="system-grid system-summary-grid">
    <section class="panel"><div class="panel-head"><h2>Домашний роутер</h2><RouterIcon class="icon" /></div><div class="panel-body"><dl class="key-values"><div><dt>Система</dt><dd>Gofro Router</dd></div><div><dt>Без перезагрузки</dt><dd>{formatDuration(status.stats.uptime_seconds)}</dd></div><div><dt>Адрес панели</dt><dd class="mono">{status.ap.domain}</dd></div></dl><a class="text-link" href={p("/analytics")}>Диагностика <ChevronRight class="icon" /></a></div></section>
    <section class="panel"><div class="panel-head"><h2>Обновления</h2></div><div class="panel-body"><div class="version">{status.version}</div><p class="muted small mb-[19px]">Установленная версия</p><button class="btn" type="button" disabled={app.busy || status.update.running} onclick={() => { app.clearActionError(); confirm = "update"; }}><RefreshCw class={status.update.running ? "icon animate-spin" : "icon"} />{status.update.running ? "Обновляем…" : "Проверить обновления"}</button><p class="small muted mt-[13px]" role="status">{updateMessage}</p></div></section>
  </div>
  <section class="panel theme-settings"><h2>Оформление</h2><div class="segmented theme-picker" role="group" aria-label="Оформление панели">{#each themes as theme (theme.value)}<button type="button" aria-pressed={appearance.theme === theme.value} onclick={() => setTheme(theme.value)}>{theme.label}</button>{/each}</div></section>
  <section class="panel"><div class="full-row"><div class="row-main"><h3>Перезагрузка</h3></div><button class="btn" type="button" disabled={app.busy || status.update.running} onclick={() => { app.clearActionError(); confirm = "reboot"; }}>Перезагрузить</button></div><div class="full-row"><div class="row-main"><h3>Вход в панель</h3></div><button class="btn ghost" type="button" disabled={app.busy} onclick={app.logoutAuth}>Выйти</button></div></section>
{/if}
{#if confirm}
  <Dialog title={confirm === "reboot" ? "Перезагрузить роутер?" : "Проверить и установить обновление?"} onclose={() => confirm = null} busy={app.busy}>
    <p class="dialog-intro">{confirm === "reboot" ? "Интернет и Wi-Fi временно отключатся. Настройки сохранятся." : "Если доступна новая версия, роутер проверит её подпись и установит обновление. Панель может временно отключиться."}</p>
    <div class="form-actions"><button class="btn ghost" type="button" disabled={app.busy} onclick={() => confirm = null}>Отмена</button><button class="btn primary" type="button" disabled={app.busy} onclick={perform}>{app.busy ? "Выполняем…" : confirm === "reboot" ? "Перезагрузить" : "Проверить и обновить"}</button></div>
    {#if app.actionError}<p class="error" role="alert">{app.actionError}</p>{/if}
  </Dialog>
{/if}
