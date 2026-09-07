<script lang="ts">
  import { onMount } from "svelte";
  import { getAppContext } from "../app-context";
  import PasswordInput from "../components/password-input.svelte";
  import Servers from "./servers.svelte";

  const app = getAppContext();
  const onboarding = $derived(app.onboarding);
  const busy = $derived(app.busy);
  const steps = [
    { key: "admin", label: "Администратор" },
    { key: "wifi", label: "Wi-Fi" },
    { key: "server", label: "VPN" },
  ] as const;
  let drafts = $state(
    (app.onboarding?.networks ?? []).map((network) => ({
      band: network.band,
      ssid: network.ssid === "GofroNET Wi-Fi Setup" || network.ssid === "OpenWrt" || !network.ssid
        ? network.band === "2g" ? "GofroNET 2G" : "GofroNET 5G"
        : network.ssid,
      password: "",
    })),
  );
  let validationError = $state("");
  let reconnecting = $state(false);
  let submittedNetworks = $state<{ band: "2g" | "5g"; ssid: string }[]>([]);
  let now = $state(Date.now());
  const windowDeadline = $derived(onboarding?.setup_window_seconds == null ? null : Date.now() + onboarding.setup_window_seconds * 1000);
  const windowExpired = $derived(windowDeadline !== null && now >= windowDeadline);
  const editableWifi = $derived(onboarding?.step === "wifi" && (!reconnecting || onboarding.error !== null || windowExpired));
  const reconnectNetworks = $derived(submittedNetworks.length ? submittedNetworks : onboarding?.networks ?? []);
  onMount(() => {
    const timer = window.setInterval(() => now = Date.now(), 1000);
    return () => window.clearInterval(timer);
  });

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
      reconnecting = true;
    }
  }

  async function finish() {
    await app.completeOnboarding();
  }

</script>

<svelte:head><title>Настройка · Gofro Router</title></svelte:head>

<main class="min-h-dvh bg-[#f5f5f5] p-4 text-[#09090b] sm:p-6">
  <section class="mx-auto grid min-h-[calc(100dvh-2rem)] max-w-4xl content-center gap-5 sm:min-h-[calc(100dvh-3rem)]" aria-labelledby="onboarding-title">
    <header class="rounded-[28px] bg-[linear-gradient(145deg,#202024,#09090b_72%)] p-5 text-white sm:p-7">
      <span class="text-xs font-bold tracking-[0.18em] text-[#aaaab1] uppercase">Gofro Router</span>
      <h1 class="mt-2 text-3xl font-extrabold tracking-[-0.06em] sm:text-4xl" id="onboarding-title">Настройка сети</h1>
      <ol class="mt-6 grid grid-cols-3 gap-2 text-[0.65rem] font-bold sm:text-xs">
        {#each steps as step, index (step.key)}
          {@const active = onboarding?.step === step.key || (step.key === "wifi" && onboarding?.step === "wifi_applying")}
          {@const done = (onboarding?.step === "wifi" || onboarding?.step === "wifi_applying" || onboarding?.step === "server" || onboarding?.step === "complete") && index === 0 || (onboarding?.step === "server" || onboarding?.step === "complete") && index === 1}
          <li class={`border-b-2 pb-2 ${active ? "border-white text-white" : done ? "border-[#777780] text-[#d6d6dc]" : "border-[#47474d] text-[#aaaab1]"}`}>{index + 1}. {step.label}</li>
        {/each}
      </ol>
    </header>

    {#if !onboarding}
      <article class="rounded-[28px] border border-[#dedee1] bg-white p-6 text-sm text-[#74747d]">Получаем состояние настройки…</article>
    {:else if editableWifi}
      <article class="rounded-[28px] border border-[#dedee1] bg-white p-5 shadow-sm sm:p-7">
        <h2 class="text-2xl font-bold tracking-[-0.04em]">Защитите Wi-Fi</h2>
        <p class="mt-2 text-sm leading-relaxed text-[#74747d]">Укажите сеть для каждого доступного диапазона. После сохранения текущая сеть настройки отключится через несколько секунд.</p>
        <form class="mt-5 grid gap-5" onsubmit={saveWifi}>
          <div class={`grid gap-4 ${drafts.length > 1 ? "sm:grid-cols-2" : ""}`}>
          {#each drafts as network (network.band)}
            <fieldset class="grid gap-3 rounded-2xl bg-[#f5f5f5] p-4"><legend class="px-1 text-xs font-bold text-[#74747d]">{network.band === "2g" ? "2,4 ГГц" : "5 ГГц"}</legend><label><span class="mb-1.5 block text-xs font-semibold text-[#74747d]">Название сети (SSID)</span><input class="h-12 w-full rounded-2xl border border-[#dedee1] bg-white px-4" bind:value={network.ssid} required maxlength="32" autocomplete="off" /></label><PasswordInput label="Пароль Wi-Fi" bind:value={network.password} required minlength={8} maxlength={63} autocomplete="new-password" disabled={busy} /></fieldset>
          {/each}
          </div>
          {#if windowExpired}<p class="rounded-2xl border border-red-200 bg-red-50 p-3 text-xs text-red-700" role="alert">Время настройки истекло; запустите команду установки в консоли роутера повторно.</p>{/if}
          {#if validationError || onboarding.error || app.actionError}<p class="rounded-2xl border border-red-200 bg-red-50 p-3 text-xs text-red-700" role="alert">{validationError || onboarding.error || app.actionError}</p>{/if}
          <button class="min-h-13 rounded-2xl bg-[#09090b] px-5 text-sm font-bold text-white disabled:opacity-60" type="submit" disabled={busy || windowExpired}>{busy ? "Сохраняем…" : "Сохранить Wi-Fi"}</button>
        </form>
      </article>
    {:else if onboarding.step === "wifi_applying" || (onboarding.step === "wifi" && reconnecting)}
      <article class="rounded-[28px] border border-[#dedee1] bg-white p-5 shadow-sm sm:p-7" role="status">
        <h2 class="text-2xl font-bold tracking-[-0.04em]">Подключитесь к новой сети</h2>
        <p class="mt-3 text-sm leading-relaxed text-[#74747d]">Сеть настройки исчезнет. Выберите новую сеть в настройках устройства, затем откройте <strong class="text-[#09090b]">https://wifi.gofro.net</strong> и войдите, если сессия сбросилась.</p>
        <p class="mt-3 text-xs leading-relaxed text-[#74747d]">Если новая сеть не появилась, а 15 минут уже прошли, повторите команду установки в консоли роутера. Пароль панели сохранится.</p>
        <ul class="mt-4 grid gap-2 text-sm font-bold">{#each reconnectNetworks as network (network.band)}<li class="rounded-2xl bg-[#f5f5f5] px-4 py-3">{network.band === "2g" ? "2,4 ГГц" : "5 ГГц"}: {network.ssid}</li>{/each}</ul>
        {#if reconnecting}<p class="mt-4 rounded-2xl bg-[#f5f5f5] p-3 text-xs leading-relaxed text-[#74747d]">Ответ мог прерваться при смене Wi-Fi. Не отправляйте данные повторно: подключитесь к новой сети и проверьте состояние.</p>{/if}
        {#if onboarding.error}<p class="mt-4 rounded-2xl border border-red-200 bg-red-50 p-3 text-xs text-red-700">{onboarding.error}</p>{/if}
        <button class="mt-5 min-h-12 rounded-2xl border border-[#dedee1] bg-white px-5 text-sm font-bold" type="button" onclick={app.loadOnboarding} disabled={app.onboardingLoading}>Проверить подключение</button>
      </article>
    {:else if onboarding.step === "server"}
      <article class="rounded-[28px] border border-[#dedee1] bg-white p-5 shadow-sm sm:p-7">
        <h2 class="text-2xl font-bold tracking-[-0.04em]">VPN, если нужен</h2>
        <p class="mt-2 text-sm leading-relaxed text-[#74747d]">Импортируйте WireGuard-профиль или подключите собственный VPS. VPN не включится автоматически.</p>
        {#if app.hasStatus}<div class="mt-5"><Servers onboarding /></div>{:else}<div class="mt-5 rounded-2xl border border-red-200 bg-red-50 p-4 text-sm text-red-700" role="alert"><p>{app.pollError || "Не удалось загрузить данные VPS."}</p><button class="mt-3 min-h-11 rounded-xl border border-red-200 bg-white px-4 text-xs font-bold" type="button" onclick={app.refresh}>Повторить</button></div>{/if}
        {#if app.actionError}<p class="mt-4 rounded-2xl border border-red-200 bg-red-50 p-3 text-xs text-red-700" role="alert">{app.actionError}</p>{/if}
        <button class="mt-5 min-h-13 w-full rounded-2xl bg-[#09090b] px-5 text-sm font-bold text-white disabled:opacity-60" type="button" onclick={finish} disabled={busy}>{busy ? "Завершаем…" : app.hasStatus && app.status.servers.length > 0 ? "Завершить настройку" : "Пропустить, настроить позже"}</button>
        <p class="mt-3 text-xs leading-relaxed text-[#74747d]">После добавления профиля нажмите эту кнопку. VPN можно включить на главной панели позже.</p>
      </article>
    {:else}
      <article class="rounded-[28px] border border-[#bde4cc] bg-[#edf9f1] p-6 text-sm text-[#185c38]">Настройка завершена. Открываем панель управления…</article>
    {/if}
  </section>
</main>
