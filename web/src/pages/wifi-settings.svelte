<script lang="ts">
  import CheckCircle2 from "lucide-svelte/icons/circle-check";
  import ExternalLink from "lucide-svelte/icons/external-link";
  import Globe2 from "lucide-svelte/icons/globe-2";
  import KeyRound from "lucide-svelte/icons/key-round";
  import RefreshCw from "lucide-svelte/icons/refresh-cw";
  import Route from "lucide-svelte/icons/route";
  import Router from "lucide-svelte/icons/router";
  import Wifi from "lucide-svelte/icons/wifi";

  import { getAppContext } from "../app-context";

  const app = getAppContext();
  const status = $derived(app.status);
  const busy = $derived(app.busy);
  const mutation = $derived(app.mutation);
  const reconnectSsid = $derived(app.reconnectSsid);
  const updateStatus = $derived(app.updateStatus);
  const updateError = $derived(app.updaterError);
  const updateAction = $derived(app.updateAction);
  const updating = $derived(app.updateActive);
  const { setMode, saveAp, resumePolling, checkUpdate, startUpdate } = app;

  let ssid = $state("");
  let password = $state("");
  let validationError = $state("");
  let initialized = $state(false);

  $effect(() => {
    if (!initialized) {
      ssid = status.ap.ssid || "GofroNET WiFi";
      initialized = true;
    }
  });

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    const nextSsid = ssid.trim();
    if (!nextSsid) {
      validationError = "Введите имя Wi-Fi сети.";
      return;
    }
    if (password && password.length < 8) {
      validationError = "Новый пароль должен содержать не менее 8 символов.";
      return;
    }
    validationError = "";
    if (await saveAp(nextSsid, password)) password = "";
  }

  async function installUpdate() {
    if (
      confirm(
        "Во время установки VPN и панель будут недоступны несколько секунд. Продолжить?",
      )
    ) {
      await startUpdate();
    }
  }
</script>

<svelte:head><title>Wi-Fi · GofroWiFi</title></svelte:head>

<section class="grid min-w-0 gap-5 lg:gap-6" aria-labelledby="wifi-title">
  <header class="min-w-0 px-0.5 py-2">
    <div class="min-w-0">
      <span class="text-xs font-bold tracking-[0.18em] text-[#74747d] uppercase"
        >Точка доступа</span
      >
      <h1
        class="mt-2 text-[clamp(2.25rem,11vw,3.25rem)] leading-[0.98] font-extrabold tracking-[-0.06em] lg:text-[clamp(3rem,5vw,4.2rem)]"
        id="wifi-title"
      >
        Настройки Wi-Fi
      </h1>
      <p class="mt-3.5 max-w-2xl text-base leading-relaxed text-[#74747d]">
        Измените имя беспроводной сети или задайте новый пароль.
      </p>
    </div>
  </header>

  {#if reconnectSsid}
    <article
      class="mx-auto flex min-h-130 w-full max-w-3xl flex-col items-center overflow-hidden rounded-[28px] bg-[linear-gradient(145deg,#202024,#09090b_72%)] px-5 py-9 text-center text-white shadow-xl shadow-black/10"
      role="status"
      aria-live="assertive"
    >
      <span
        class="mb-5 grid size-16 place-items-center rounded-[18px] bg-white text-[#09090b]"
        ><CheckCircle2 size={32} /></span
      >
      <span class="text-xs font-bold tracking-[0.18em] text-[#aaaab1] uppercase"
        >Настройки применены</span
      >
      <h2 class="my-2.5 text-3xl font-bold tracking-tighter">
        Подключитесь заново
      </h2>
      <p class="my-2 max-w-lg text-sm leading-relaxed text-[#aaaab1]">
        Точка доступа перезапускается. Откройте настройки Wi-Fi на телефоне и
        выберите сеть:
      </p>
      <strong
        class="my-2 flex min-h-14 w-full max-w-md items-center justify-center gap-2.5 rounded-2xl border border-[#3b3b40] bg-[#202024] px-4"
        ><Wifi size={22} />{reconnectSsid}</strong
      >
      <p class="my-2 max-w-lg text-sm leading-relaxed text-[#aaaab1]">
        После подключения вернитесь по локальному адресу:
      </p>
      <a
        class="mb-5 flex min-h-14 w-full max-w-md items-center justify-center gap-2.5 rounded-2xl border border-[#3b3b40] bg-[#202024] px-4 font-mono text-xs text-white no-underline"
        href="http://gofrowifi.net"
        ><Globe2 size={19} />http://gofrowifi.net<ExternalLink size={16} /></a
      >
      <button
        class="min-h-13 w-full max-w-md rounded-2xl border border-white bg-white px-5 text-sm font-bold text-[#09090b]"
        type="button"
        onclick={resumePolling}>Я подключился</button
      >
      <small
        class="mt-3.5 max-w-md text-[0.68rem] leading-relaxed text-[#aaaab1]"
        >Автообновление приостановлено, поэтому это сообщение останется на
        экране.</small
      >
    </article>
  {:else}
    <article
      class="grid min-w-0 gap-6 overflow-hidden rounded-[28px] bg-[linear-gradient(145deg,#202024,#09090b_72%)] p-6 text-white shadow-xl shadow-black/10 lg:grid-cols-[minmax(0,1fr)_250px] lg:items-center lg:p-7"
    >
      <div class="flex min-w-0 items-start gap-3.5">
        <Route class="shrink-0" size={21} />
        <div class="min-w-0">
          <span class="text-xs text-[#aaaab1]">Маршрутизация</span>
          <h2 class="mt-1 text-xl font-bold tracking-[-0.035em]">
            {status.vpn_enabled ? "Через VPN" : "Напрямую"}
          </h2>
          <p class="mt-2 text-xs leading-relaxed text-[#aaaab1]">
            {status.vpn_enabled
              ? "Игровой трафик идёт через выбранный сервер."
              : "VPN обойдён, трафик выходит через домашний роутер."}
          </p>
        </div>
      </div>
      <div class="grid grid-cols-2 gap-2.5" aria-label="Режим маршрутизации">
        <button
          class={`min-h-13 rounded-2xl border text-xs font-bold ${status.vpn_enabled ? "border-white bg-white text-[#09090b]" : "border-[#3c3c41] bg-[#202024] text-[#aaaab1]"}`}
          type="button"
          disabled={busy}
          aria-pressed={status.vpn_enabled}
          onclick={() => setMode(true)}
          >{mutation === "mode" && status.vpn_enabled
            ? "Подключение…"
            : "VPN"}</button
        >
        <button
          class={`min-h-13 rounded-2xl border text-xs font-bold ${!status.vpn_enabled ? "border-white bg-white text-[#09090b]" : "border-[#3c3c41] bg-[#202024] text-[#aaaab1]"}`}
          type="button"
          disabled={busy}
          aria-pressed={!status.vpn_enabled}
          onclick={() => setMode(false)}>DIRECT</button
        >
      </div>
    </article>

    <div
      class="grid min-w-0 gap-3 lg:grid-cols-[minmax(0,1.2fr)_minmax(300px,0.8fr)] lg:items-start"
    >
      <article
        class="min-w-0 overflow-hidden rounded-[28px] border border-[#dedee1] bg-white p-5 shadow-sm sm:p-6"
      >
        <div class="mb-5">
          <span
            class="text-xs font-bold tracking-[0.18em] text-[#74747d] uppercase"
            >Основная сеть</span
          >
          <h2 class="mt-1.5 text-xl font-bold tracking-[-0.035em]">
            Параметры точки
          </h2>
        </div>
        <form class="grid gap-4.5" onsubmit={submit}>
          <label>
            <span class="mb-2 block text-xs font-semibold text-[#74747d]"
              >Имя сети (SSID)</span
            >
            <div class="relative">
              <Wifi
                class="pointer-events-none absolute left-4 top-4.5 text-[#74747d]"
                size={19}
              /><input
                class="h-14 w-full rounded-2xl border border-[#dedee1] bg-white pl-12 pr-4 text-base"
                bind:value={ssid}
                required
                maxlength="32"
                autocomplete="off"
                placeholder="GofroNET WiFi"
              />
            </div>
          </label>
          <label>
            <span class="mb-2 block text-xs font-semibold text-[#74747d]"
              >Новый пароль</span
            >
            <div class="relative">
              <KeyRound
                class="pointer-events-none absolute left-4 top-4.5 text-[#74747d]"
                size={19}
              /><input
                class="h-14 w-full rounded-2xl border border-[#dedee1] bg-white pl-12 pr-4 text-base"
                bind:value={password}
                type="password"
                minlength="8"
                maxlength="63"
                autocomplete="new-password"
                placeholder="Не менять"
              />
            </div>
            <small class="mt-2 block text-xs leading-relaxed text-[#74747d]"
              >Оставьте поле пустым, чтобы сохранить текущий пароль.</small
            >
          </label>
          {#if validationError}<p
              class="m-0 rounded-2xl border border-red-200 bg-red-50 p-3 text-xs leading-relaxed text-red-700"
              role="alert"
            >
              {validationError}
            </p>{/if}
          <button
            class="mt-1 min-h-13 w-full rounded-2xl border border-[#09090b] bg-[#09090b] px-5 text-sm font-bold text-white"
            type="submit"
            disabled={busy}
            >{mutation === "ap"
              ? "Применяем…"
              : "Сохранить и перезапустить"}</button
          >
        </form>
      </article>

      <aside class="grid min-w-0 gap-3">
        <article
          class="min-w-0 overflow-hidden rounded-[28px] border border-[#dedee1] bg-white p-5 shadow-sm sm:p-6"
        >
          <div
            class="mb-4.5 grid size-12.5 place-items-center rounded-2xl bg-[#f0f0f2]"
          >
            <Router size={22} />
          </div>
          <span class="text-xs text-[#74747d]">Локальная сеть</span>
          <h2
            class="mb-5 mt-1.5 wrap-break-word text-xl font-bold tracking-[-0.04em]"
          >
            {status.ap.ssid || "GofroNET WiFi"}
          </h2>
          <dl class="m-0">
            <div
              class="flex justify-between gap-3 border-t border-[#ececef] py-3.5"
            >
              <dt class="text-xs text-[#74747d]">Шлюз</dt>
              <dd class="m-0 wrap-break-word text-right font-mono text-xs">
                {status.ap.address || "Нет данных"}
              </dd>
            </div>
            <div
              class="flex justify-between gap-3 border-t border-[#ececef] py-3.5"
            >
              <dt class="text-xs text-[#74747d]">Домен</dt>
              <dd class="m-0 wrap-break-word text-right font-mono text-xs">
                {status.ap.domain || "gofrowifi.net"}
              </dd>
            </div>
          </dl>
        </article>
        <article
          class="min-w-0 overflow-hidden rounded-[28px] border border-[#dedee1] bg-white p-5 shadow-sm sm:p-6"
          aria-labelledby="update-title"
        >
          <div
            class="mb-4.5 grid size-12.5 place-items-center rounded-2xl bg-[#f0f0f2]"
          >
            <RefreshCw class={updating ? "animate-spin" : ""} size={22} />
          </div>
          <span class="text-xs text-[#74747d]">Система</span>
          <h2
            class="mb-1.5 mt-1 text-xl font-bold tracking-[-0.04em]"
            id="update-title"
          >
            Обновление ПО
          </h2>
          <p class="m-0 text-xs leading-relaxed text-[#74747d]">
            Текущая версия: <strong class="text-[#09090b]"
              >{updateStatus?.installed_version ?? status.version}</strong
            >
          </p>
          <p
            class={`mb-4 mt-3 text-xs leading-relaxed ${updateStatus?.state === "error" || updateError ? "text-red-700" : "text-[#74747d]"}`}
            role={updateStatus?.state === "error" || updateError
              ? "alert"
              : undefined}
          >
            {updateError || app.updateText}
          </p>
          {#if updating}
            <progress
              class="mb-1 h-2 w-full overflow-hidden rounded-full accent-[#09090b]"
              aria-label={app.updateText}
            ></progress>
          {:else if updateStatus?.state === "available"}
            <button
              class="min-h-12 w-full rounded-2xl border border-[#09090b] bg-[#09090b] px-4 text-xs font-bold text-white"
              type="button"
              disabled={busy}
              onclick={installUpdate}
              >{updateAction === "start"
                ? "Запускаем…"
                : `Установить ${updateStatus.version}`}</button
            >
          {:else}
            <button
              class="min-h-12 w-full rounded-2xl border border-[#09090b] bg-[#09090b] px-4 text-xs font-bold text-white"
              type="button"
              disabled={busy}
              onclick={checkUpdate}
              >{updateAction === "check"
                ? "Проверяем…"
                : updateStatus?.state === "success"
                  ? "Проверить снова"
                  : "Проверить обновления"}</button
            >
          {/if}
        </article>
        <article
          class="flex min-w-0 gap-3 rounded-[20px] border border-[#dedee1] bg-white p-4"
        >
          <KeyRound class="shrink-0" size={19} />
          <div class="min-w-0">
            <strong class="text-sm">Пароль защищен</strong>
            <p class="mt-1.5 text-xs leading-relaxed text-[#74747d]">
              Текущий пароль никогда не запрашивается и не отображается в
              панели.
            </p>
          </div>
        </article>
      </aside>
    </div>
  {/if}
</section>
