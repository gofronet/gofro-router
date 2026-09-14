<script lang="ts">
  import RefreshCw from "lucide-svelte/icons/refresh-cw";
  import { getAppContext } from "../app-context";
  import { appearance, setTheme, type Theme } from "../stores/theme.svelte";
  import Dialog from "../components/dialog.svelte";
  import ChangePassword from "../components/change-password.svelte";
  const app = getAppContext();
  const status = $derived(app.status);
  const themes: { value: Theme; label: string }[] = [{ value: "light", label: "Светлая" }, { value: "dark", label: "Тёмная" }, { value: "system", label: "Системная" }];
  let confirm = $state(false);
  const updateMessage = $derived(status.update.running ? "Загружаем и устанавливаем обновление. Панель может ненадолго отключиться." : status.update.result === "current" ? "Обновлений нет." : status.update.result === "failed" ? "Не удалось обновить. Проверьте интернет и попробуйте снова." : "");

  async function perform() {
    if (app.busy) return;
    await app.startUpdate();
    if (!app.actionError) confirm = false;
  }
</script>

<svelte:head><title>Панель · Gofro VPN</title></svelte:head>
<section class="panel"><div class="panel-head"><h2>Обновления Gofro</h2></div><div class="panel-body"><div class="version">{status.version}</div><button class="btn" type="button" disabled={app.busy || status.update.running} onclick={() => { app.clearActionError(); confirm = true; }}><RefreshCw class={status.update.running ? "icon animate-spin" : "icon"} />{status.update.running ? "Обновляем…" : "Проверить обновления"}</button><div class="full-row mt-5"><div class="row-main"><h3>Автообновление</h3><p>Проверять и устанавливать обновления без подтверждения.</p></div><button class="switch" type="button" role="switch" aria-checked={status.update.auto_update_enabled ?? false} aria-label="Автообновление" disabled={app.busy || status.update.running} onclick={() => void app.setAutoUpdate(!(status.update.auto_update_enabled ?? false))}><span class="switch-track"></span></button></div>{#if updateMessage}<p class="small muted mt-[13px]" role="status">{updateMessage}</p>{/if}</div></section>
<section class="panel theme-settings"><h2>Оформление</h2><div class="segmented theme-picker" role="group" aria-label="Оформление панели">{#each themes as theme (theme.value)}<button type="button" aria-pressed={appearance.theme === theme.value} onclick={() => setTheme(theme.value)}>{theme.label}</button>{/each}</div></section>
<ChangePassword />
<section class="panel"><div class="full-row"><div class="row-main"><h3>Вход в панель</h3></div><button class="btn ghost" type="button" disabled={app.busy} onclick={app.logoutAuth}>Выйти</button></div></section>
{#if confirm}
  <Dialog title="Проверить и установить обновление?" onclose={() => confirm = false} busy={app.busy}>
    <p class="dialog-intro">Если доступна новая версия, Gofro проверит её подпись и установит обновление. Панель может временно отключиться.</p>
    <div class="form-actions"><button class="btn ghost" type="button" disabled={app.busy} onclick={() => confirm = false}>Отмена</button><button class="btn primary" type="button" disabled={app.busy} onclick={perform}>{app.busy ? "Выполняем…" : "Проверить и обновить"}</button></div>
    {#if app.actionError}<p class="error" role="alert">{app.actionError}</p>{/if}
  </Dialog>
{/if}
