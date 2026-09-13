<script lang="ts">
  import ChevronRight from "lucide-svelte/icons/chevron-right";
  import Server from "lucide-svelte/icons/server";
  import Upload from "lucide-svelte/icons/upload";
  import { getAppContext } from "../app-context";
  import ServerDialogs, { type ServerDialogFlow } from "../components/server-dialogs.svelte";
  import OperationWarning from "../components/operation-warning.svelte";

  const app = getAppContext();
  const onboarding = $derived(app.onboarding);
  const busy = $derived(app.busy);
  const hasServer = $derived(app.hasStatus && app.status.servers.length > 0);
  let flow = $state<ServerDialogFlow>(null);

  async function finish() {
    if (!busy) await app.completeOnboarding();
  }
</script>

<svelte:head><title>Настройка · Gofro VPN</title></svelte:head>

<main class="access-page">
  <section class="access-card" aria-labelledby="onboarding-title">
    <div class="brand access-brand"><strong>Gofro</strong><span>VPN</span></div>
    <ol class="setup-progress" aria-label="Ход настройки"><li class:done={onboarding?.step === "server" || onboarding?.step === "complete"} class:current={onboarding?.step === "admin"}><span>1</span>Пароль</li><li class:current={onboarding?.step === "server"}><span>2</span>VPN</li></ol>
    <div class="access-copy">
      <OperationWarning />
      {#if !onboarding}
        <h1 id="onboarding-title">Настройка Gofro VPN</h1><p class="access-note">Получаем состояние настройки...</p>
      {:else if onboarding.step === "server"}
        <h1 id="onboarding-title">Подключите VPN</h1>
        {#if app.hasStatus}
          <div class="setup-choices">
            <button class="choice-button" type="button" disabled={busy} onclick={() => flow = { kind: "import" }}><Upload size={20} /><span><strong>Импортировать настройки</strong><small>Файл WireGuard (.conf)</small></span><ChevronRight size={20} /></button>
            <button class="choice-button" type="button" disabled={busy} onclick={() => flow = { kind: "vps" }}><Server size={20} /><span><strong>Подключить свой сервер</strong></span><ChevronRight size={20} /></button>
          </div>
        {:else}
          <p class="notice">Не удалось загрузить данные VPN.</p><button class="btn ghost access-submit" type="button" onclick={app.refresh}>Повторить</button>
        {/if}
        {#if app.actionError}<p class="error" role="alert">{app.actionError}</p>{/if}
        <button class="btn ghost access-submit" type="button" onclick={finish} disabled={busy}>{busy ? "Завершаем..." : hasServer ? "Завершить настройку" : "Настроить позже"}</button>
      {:else}
        <h1 id="onboarding-title">Требуется миграция OpenWrt</h1><p class="access-note" role="alert">Сервер вернул устаревший этап настройки сети. Обновите Gofro VPN: панель не изменяет настройки OpenWrt.</p>
      {/if}
    </div>
  </section>
</main>

{#if app.hasStatus}<ServerDialogs bind:flow />{/if}
