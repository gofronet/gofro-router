<script lang="ts">
  import { onMount } from "svelte";
  import ChevronRight from "lucide-svelte/icons/chevron-right";
  import Server from "lucide-svelte/icons/server";
  import Upload from "lucide-svelte/icons/upload";
  import { getAppContext } from "../app-context";
  import PasswordInput from "../components/password-input.svelte";
  import ServerDialogs, { type ServerDialogFlow } from "../components/server-dialogs.svelte";

  type WifiDraft = { band: "2g" | "5g"; ssid: string; password: string };

  const app = getAppContext();
  const onboarding = $derived(app.onboarding);
  const busy = $derived(app.busy);
  const hasServer = $derived(app.hasStatus && app.status.servers.length > 0);
  let drafts = $state<WifiDraft[]>([]);
  let submittedNetworks = $state<Pick<WifiDraft, "band" | "ssid">[]>([]);
  let validationError = $state("");
  let now = $state(Date.now());
  let deadline = $state<number | null>(null);
  let deadlineWindow = "";
  let flow = $state<ServerDialogFlow>(null);
  let finishAfterAdd = $state(false);

  $effect(() => {
    if (onboarding?.step === "wifi" && drafts.length === 0) {
      drafts = onboarding.networks.map((network) => ({
        band: network.band,
        ssid: network.ssid === "GofroNET Wi-Fi Setup" || network.ssid === "OpenWrt" || !network.ssid
          ? ""
          : network.ssid,
        password: "",
      }));
    }
    const window = onboarding?.step === "wifi" ? onboarding.setup_window_seconds : null;
    const key = window === null ? "" : `${window}`;
    if (key !== deadlineWindow) {
      deadlineWindow = key;
      deadline = window === null ? null : Date.now() + window * 1_000;
    }
  });

  onMount(() => {
    const timer = window.setInterval(() => now = Date.now(), 1_000);
    return () => window.clearInterval(timer);
  });

  const windowExpired = $derived(deadline !== null && now >= deadline);
  const reconnectNetworks = $derived(submittedNetworks.length ? submittedNetworks : onboarding?.networks ?? []);
  const reconnecting = $derived(Boolean(app.reconnectSsid && onboarding?.step === "wifi"));

  function validSsid(value: string): boolean {
    return value.length > 0 && new TextEncoder().encode(value).length <= 32 && !/[\x00-\x1f\x7f]/.test(value);
  }

  function validPassword(value: string): boolean {
    return /^[\x20-\x7e]{8,63}$/.test(value);
  }

  async function saveWifi(event: SubmitEvent) {
    event.preventDefault();
    if (!onboarding || busy || windowExpired) return;
    if (drafts.length !== onboarding.networks.length || drafts.some((network) => !validSsid(network.ssid) || !validPassword(network.password))) {
      validationError = "SSID: от 1 до 32 байт без управляющих символов. Пароль: 8-63 печатных ASCII-символа.";
      return;
    }
    validationError = "";
    submittedNetworks = drafts.map(({ band, ssid }) => ({ band, ssid }));
    const result = await app.saveOnboardingWifi({ networks: drafts });
    if (result !== "error") {
      drafts = drafts.map((network) => ({ ...network, password: "" }));
    }
  }

  async function finish() {
    if (busy) return;
    await app.completeOnboarding();
  }

  async function completeAfterAdd() {
    finishAfterAdd = true;
    await finish();
  }
</script>

<svelte:head><title>Настройка · Gofro Router</title></svelte:head>

<main class="access-page">
  <section class="access-card" aria-labelledby="onboarding-title">
    <div class="brand access-brand"><strong>Gofro</strong><span>Router</span></div>
    <ol class="setup-progress" aria-label="Ход настройки">
      <li class:done={onboarding?.step !== "admin"} class:current={onboarding?.step === "admin"}><span>1</span>Пароль</li>
      <li class:done={onboarding?.step === "server" || onboarding?.step === "complete"} class:current={onboarding?.step === "wifi" || onboarding?.step === "wifi_applying"}><span>2</span>Wi-Fi</li>
      <li class:current={onboarding?.step === "server"}><span>3</span>VPN</li>
    </ol>
    <div class="access-copy">
      {#if !onboarding}
        <h1 id="onboarding-title">Настройка сети</h1><p class="access-note">Получаем состояние настройки…</p>
      {:else if onboarding.step === "wifi" && !reconnecting}
        <h1 id="onboarding-title">Настройте Wi-Fi</h1>
        <p class="access-note">Задайте имя сети и пароль.</p>
        <form onsubmit={saveWifi}>
          <div class="setup-fields">
            {#each drafts as network (network.band)}
              <label class="field">{network.band === "2g" ? "2,4 ГГц" : "5 ГГц"}<input bind:value={network.ssid} required maxlength="32" autocomplete="off" placeholder="Имя сети" disabled={busy || windowExpired} /></label>
              <PasswordInput label={`Пароль ${network.band === "2g" ? "2,4 ГГц" : "5 ГГц"}`} bind:value={network.password} required minlength={8} maxlength={63} autocomplete="new-password" disabled={busy || windowExpired} />
            {/each}
          </div>
          {#if windowExpired}<p class="error" role="alert">Время настройки истекло; запустите команду установки в консоли роутера повторно.</p>{/if}
          {#if validationError || onboarding.error || app.actionError}<p class="error" role="alert">{validationError || onboarding.error || app.actionError}</p>{/if}
          <button class="btn primary access-submit" type="submit" disabled={busy || windowExpired}>{busy ? "Сохраняем…" : "Продолжить"}</button>
        </form>
      {:else if onboarding.step === "wifi_applying" || reconnecting}
        <h1 id="onboarding-title">Подключитесь к новой сети</h1>
        <p class="access-note">Сеть настройки исчезнет. Выберите новую сеть, затем откройте <strong>https://wifi.gofro.net</strong> и войдите, если сессия сбросилась.</p>
        <ul class="notice">{#each reconnectNetworks as network (network.band)}<li>{network.band === "2g" ? "2,4 ГГц" : "5 ГГц"}: {network.ssid}</li>{/each}</ul>
        {#if reconnecting}<p class="notice">Ответ мог прерваться при смене Wi-Fi. Не отправляйте данные повторно: подключитесь к новой сети и проверьте состояние.</p>{/if}
        {#if onboarding.error || app.actionError}<p class="error" role="alert">{onboarding.error || app.actionError}</p>{/if}
        <button class="btn primary access-submit" type="button" onclick={app.loadOnboarding} disabled={app.onboardingLoading}>Проверить подключение</button>
      {:else if onboarding.step === "server"}
        <h1 id="onboarding-title">Подключите VPN</h1>
        <p class="access-note">Можно добавить сейчас или сделать это позже.</p>
        {#if app.hasStatus}
          <div class="setup-choices">
            <button class="choice-button" type="button" disabled={busy} onclick={() => flow = { kind: "import" }}><Upload size={20} /><span><strong>Импортировать настройки</strong><small>Файл WireGuard (.conf)</small></span><ChevronRight size={20} /></button>
            <button class="choice-button" type="button" disabled={busy} onclick={() => flow = { kind: "vps" }}><Server size={20} /><span><strong>Подключить свой сервер</strong><small>Вход по логину и паролю</small></span><ChevronRight size={20} /></button>
          </div>
        {:else}
          <p class="notice">Не удалось загрузить данные VPN. Подключитесь к новой сети и повторите попытку.</p>
          <button class="btn ghost access-submit" type="button" onclick={app.refresh}>Повторить</button>
        {/if}
        {#if app.actionError}<p class="error" role="alert">{app.actionError}</p>{/if}
        {#if finishAfterAdd && hasServer}<p class="notice">Сервер добавлен. Завершите настройку, когда подключение восстановится.</p>{/if}
        <button class="btn ghost access-submit" type="button" onclick={finish} disabled={busy}>{busy ? "Завершаем…" : hasServer ? "Завершить настройку" : "Настроить позже"}</button>
      {:else}
        <h1 id="onboarding-title">Настройка сети</h1><p class="access-note">Проверяем состояние настройки…</p>
      {/if}
    </div>
  </section>
</main>

{#if app.hasStatus}<ServerDialogs bind:flow onadded={completeAfterAdd} />{/if}
