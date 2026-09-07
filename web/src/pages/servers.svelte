<script lang="ts">
  import { tick } from "svelte";
  import Check from "lucide-svelte/icons/check";
  import Download from "lucide-svelte/icons/download";
  import Pencil from "lucide-svelte/icons/pencil";
  import Plus from "lucide-svelte/icons/plus";
  import RefreshCw from "lucide-svelte/icons/refresh-cw";
  import ServerIcon from "lucide-svelte/icons/server";
  import ShieldCheck from "lucide-svelte/icons/shield-check";
  import Trash2 from "lucide-svelte/icons/trash-2";
  import UserPlus from "lucide-svelte/icons/user-plus";
  import X from "lucide-svelte/icons/x";

  import { getAppContext } from "../app-context";
  import PasswordInput from "../components/password-input.svelte";
  import type { Server, ServerProbe, ServerVersion } from "../domain/models";
  import { shortKey } from "../format";

  type Dialog = "import" | "new" | "edit" | "profile" | null;

  let { onboarding = false }: { onboarding?: boolean } = $props();

  const app = getAppContext();
  const status = $derived(app.status);
  const busy = $derived(app.busy);
  const mutation = $derived(app.mutation);
  const actionError = $derived(app.actionError);
  let dialog = $state<Dialog>(null);
  let editing = $state<Server | null>(null);
  let name = $state("");
  let endpoint = $state("");
  let publicKey = $state("");
  let profile = $state("");
  let host = $state("");
  let port = $state("22");
  let password = $state("");
  let probe = $state<ServerProbe | null>(null);
  let serverResults = $state<Record<string, ServerVersion>>({});
  let friendProfile = $state("");
  let validationError = $state("");
  let nameInput = $state<HTMLInputElement>();

  const activeServer = $derived(
    status.servers.find((server) => server.public_key === status.active_server_key),
  );

  function clearForm() {
    editing = null;
    name = "";
    endpoint = "";
    publicKey = "";
    profile = "";
    host = "";
    port = "22";
    password = "";
    probe = null;
    friendProfile = "";
    validationError = "";
  }

  async function openForm(kind: Exclude<Dialog, "profile">, server: Server | null = null) {
    clearForm();
    app.clearActionError();
    dialog = kind;
    editing = server;
    name = server?.name || "";
    endpoint = server?.endpoint || "";
    publicKey = server?.public_key || "";
    await tick();
    nameInput?.focus();
  }

  function closeDialog() {
    if (!busy) {
      clearForm();
      dialog = null;
    }
  }

  async function saveImport(event: SubmitEvent) {
    event.preventDefault();
    const values = { name: name.trim(), profile: profile.trim() };
    if (!values.name || !values.profile) {
      validationError = "Заполните название и вставьте WireGuard-профиль.";
      return;
    }
    validationError = "";
    if (await app.importServer(values)) {
      closeDialog();
    }
  }

  async function saveEdit(event: SubmitEvent) {
    event.preventDefault();
    if (!editing || !name.trim()) {
      validationError = "Введите название профиля.";
      return;
    }
    const values = {
      name: name.trim(),
      endpoint: editing.managed ? editing.endpoint : endpoint.trim(),
      public_key: editing.managed ? editing.public_key : publicKey.trim(),
    };
    if (!values.endpoint || !values.public_key) {
      validationError = "Заполните endpoint и публичный ключ.";
      return;
    }
    validationError = "";
    if (await app.updateServer(editing.public_key, values)) closeDialog();
  }

  async function probeServer(event: SubmitEvent) {
    event.preventDefault();
    const sshPort = Number(port);
    if (!name.trim() || !host.trim() || !password || !Number.isInteger(sshPort) || sshPort < 1 || sshPort > 65535) {
      validationError = "Заполните название, IP-адрес, SSH-порт и пароль root.";
      return;
    }
    validationError = "";
    probe = await app.probeServer(host.trim(), sshPort);
  }

  async function bootstrap() {
    if (!probe) return;
    if (!password) {
      probe = null;
      validationError = "Введите пароль root заново и повторите проверку VPS.";
      return;
    }
    try {
      if (await app.bootstrapServer(name.trim(), probe.host, probe.port, password, probe.host_key)) {
        closeDialog();
      } else probe = null;
    } finally {
      password = "";
    }
  }

  async function selectServer(server: Server) {
    if (server.public_key !== status.active_server_key) await app.selectServer(server.public_key);
  }

  async function deleteServer(server: Server) {
    const suffix = server.managed ? " VPS продолжит работать." : "";
    if (confirm(`Удалить профиль «${server.name}»?${suffix}`)) await app.removeServer(server.public_key);
  }

  async function checkServer(server: Server) {
    const result = await app.checkServer(server.public_key);
    if (result) serverResults = { ...serverResults, [server.public_key]: result };
  }

  async function updateServer(server: Server) {
    if (!confirm(`Обновить Gofro на VPS «${server.name}»?`)) return;
    const result = await app.updateManagedServer(server.public_key);
    if (result) serverResults = { ...serverResults, [server.public_key]: result };
  }

  async function createProfile(server: Server) {
    const result = await app.createFriendProfile(server.public_key);
    if (result) {
      friendProfile = result.profile;
      dialog = "profile";
    }
  }

  function downloadProfile() {
    const url = URL.createObjectURL(new Blob([friendProfile], { type: "text/plain" }));
    const link = document.createElement("a");
    link.href = url;
    link.download = "gofro-friend.conf";
    link.click();
    URL.revokeObjectURL(url);
  }

  function handleDialogKeydown(event: KeyboardEvent) {
    if (event.key === "Escape") closeDialog();
  }
</script>

<svelte:head><title>Серверы · Gofro Router</title></svelte:head>

<section class="grid min-w-0 gap-5 lg:gap-6" aria-labelledby="servers-title">
  <header class="min-w-0 px-0.5 py-2 sm:flex sm:items-end sm:justify-between sm:gap-6">
    <div class="min-w-0">
      <span class="text-xs font-bold tracking-[0.18em] text-[#74747d] uppercase">VPN-профили</span>
      <h1 class="mt-2 text-[clamp(2.25rem,11vw,3.25rem)] leading-[0.98] font-extrabold tracking-[-0.06em] lg:text-[clamp(3rem,5vw,4.2rem)]" id="servers-title">Конфигурации</h1>
      <p class="mt-3.5 max-w-2xl text-base leading-relaxed text-[#74747d]">Выберите маршрут, импортируйте профиль или подключите новый VPS.</p>
    </div>
    <div class="mt-4 grid grid-cols-2 gap-2 sm:mb-1 sm:mt-0">
      <button class="flex min-h-13 items-center justify-center gap-2 rounded-2xl border border-[#dedee1] bg-white px-4 text-sm font-bold" type="button" onclick={() => openForm("import")}><Plus size={18} />Импорт</button>
      <button class="flex min-h-13 items-center justify-center gap-2 rounded-2xl border border-[#09090b] bg-[#09090b] px-4 text-sm font-bold text-white" type="button" onclick={() => openForm("new")}><ServerIcon size={18} />Новый VPS</button>
    </div>
  </header>

  {#if !onboarding}<article class="grid min-h-48 min-w-0 grid-cols-[3.125rem_minmax(0,1fr)] items-center gap-4 overflow-hidden rounded-[28px] bg-[linear-gradient(145deg,#202024,#09090b_72%)] p-6 text-white shadow-xl shadow-black/10 lg:grid-cols-[3.125rem_minmax(0,1fr)_auto] lg:p-7">
    <div class="grid size-12.5 place-items-center rounded-2xl bg-white text-[#09090b]"><ServerIcon size={22} /></div>
    <div class="min-w-0"><span class="text-xs text-[#aaaab1]">Активный профиль</span><h2 class="my-1.5 text-2xl font-bold tracking-[-0.045em]">{activeServer?.name || "Сервер не выбран"}</h2><p class="m-0 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[0.7rem] text-[#aaaab1]">{activeServer?.endpoint || "Выберите профиль из списка ниже."}</p></div>
    <span class={`col-span-2 flex min-h-12 items-center gap-2 border-t border-[#343438] pt-3 text-xs font-bold lg:col-span-1 lg:min-w-40 lg:border-0 lg:pt-0 ${status.tunnel_active ? "text-white" : "text-red-300"}`}><i class="size-2 bg-current"></i>{status.tunnel_active ? "Туннель активен" : "Нет соединения"}</span>
  </article>

  <section class="mt-2 grid min-w-0 gap-3" aria-labelledby="profiles-title">
    <header class="flex min-w-0 items-end justify-between gap-3 px-1"><div class="min-w-0"><span class="text-xs font-bold tracking-[0.18em] text-[#74747d] uppercase">Сохраненные маршруты</span><h2 class="mt-1.5 text-xl font-bold tracking-[-0.035em]" id="profiles-title">Профили серверов</h2></div><span class="shrink-0 text-xs font-semibold text-[#74747d]">{status.servers.length} всего</span></header>
    {#if status.servers.length === 0}
      <div class="flex min-h-52 flex-col items-center justify-center gap-2 rounded-[28px] border border-[#dedee1] bg-white p-8 text-center text-[#74747d] shadow-sm"><ServerIcon size={28} /><strong class="mt-1 text-[#09090b]">Профилей пока нет</strong><span class="max-w-sm text-sm leading-relaxed">Добавьте первый VPN-сервер, чтобы включить защищенный маршрут.</span></div>
    {:else}
      <div class="grid min-w-0 gap-3 sm:grid-cols-2 xl:grid-cols-3">
        {#each status.servers as server (server.public_key)}
          {@const active = server.public_key === status.active_server_key}
          {@const result = serverResults[server.public_key]}
          <article class={`min-w-0 overflow-hidden rounded-[28px] border bg-white shadow-sm ${active ? "border-[#09090b] ring-1 ring-[#09090b]" : "border-[#dedee1]"}`}>
            <button class="grid min-h-32 w-full min-w-0 grid-cols-[1.375rem_minmax(0,1fr)] items-start gap-3 border-0 bg-transparent px-4 py-5 text-left min-[420px]:grid-cols-[1.375rem_minmax(0,1fr)_auto]" type="button" aria-pressed={active} disabled={busy || active} onclick={() => selectServer(server)}>
              <span class={`grid size-5.5 place-items-center rounded-[7px] border ${active ? "border-[#09090b] bg-[#09090b] text-white" : "border-[#aaaab0]"}`}>{#if active}<Check size={16} />{/if}</span>
              <span class="min-w-0"><strong class="block overflow-hidden text-ellipsis whitespace-nowrap text-sm">{server.name}</strong><small class="mt-1.5 block overflow-hidden text-ellipsis whitespace-nowrap text-xs text-[#74747d]">{server.endpoint}</small><code class="mt-2.5 block overflow-hidden text-ellipsis whitespace-nowrap text-[0.62rem] text-[#a0a0a7]" title={server.public_key}>{shortKey(server.public_key)}</code><span class={`mt-2 inline-flex rounded-full px-2 py-1 text-[0.62rem] font-bold ${server.managed ? "bg-[#e9f7ef] text-[#167044]" : "bg-[#f0f0f2] text-[#5a5a62]"}`}>{server.managed ? "Управляемый" : "Импортированный"}</span></span>
              <span class="col-start-2 mt-1 justify-self-start text-[0.68rem] font-bold min-[420px]:col-auto min-[420px]:mt-0 min-[420px]:self-center">{mutation === `select:${server.public_key}` ? "Подключаем…" : active ? "Активен" : "Выбрать"}</span>
            </button>
            {#if result}<p class="mx-4 mb-3 rounded-xl bg-[#f0f0f2] p-2.5 text-xs text-[#36363c]" role="status">Версия {result.version}. {result.update_available ? "Доступно обновление." : "Обновлений нет."}</p>{/if}
            <div class="grid grid-cols-2 divide-x divide-y divide-[#ececef] border-t border-[#ececef]">
              <button class="flex min-h-13 items-center justify-center gap-2 border-0 bg-transparent text-xs font-semibold text-[#74747d]" type="button" disabled={busy} onclick={() => openForm("edit", server)}><Pencil size={17} />Изменить</button>
              {#if server.managed}
                <button class="flex min-h-13 items-center justify-center gap-2 border-0 bg-transparent text-xs font-semibold text-[#74747d]" type="button" disabled={busy} onclick={() => checkServer(server)}><RefreshCw size={17} />{mutation === `check:${server.public_key}` ? "Проверяем…" : "Проверить"}</button>
                <button class="flex min-h-13 items-center justify-center gap-2 border-0 bg-transparent text-xs font-semibold text-[#74747d]" type="button" disabled={busy} onclick={() => updateServer(server)}><ShieldCheck size={17} />{mutation === `update:${server.public_key}` ? "Обновляем…" : "Обновить"}</button>
                <button class="flex min-h-13 items-center justify-center gap-2 border-0 bg-transparent text-xs font-semibold text-[#74747d]" type="button" disabled={busy} onclick={() => createProfile(server)}><UserPlus size={17} />Другу</button>
              {:else}
                <button class="flex min-h-13 items-center justify-center gap-2 border-0 bg-transparent text-xs font-semibold text-red-700" type="button" disabled={busy} onclick={() => deleteServer(server)}><Trash2 size={17} />Удалить</button>
              {/if}
              {#if server.managed}<button class="col-span-2 flex min-h-13 items-center justify-center gap-2 border-0 bg-transparent text-xs font-semibold text-red-700" type="button" disabled={busy} onclick={() => deleteServer(server)}><Trash2 size={17} />Удалить</button>{/if}
            </div>
          </article>
        {/each}
      </div>
    {/if}
  </section>{/if}
</section>

{#if dialog}
  <div class="fixed inset-0 z-100 flex items-end justify-center pt-[env(safe-area-inset-top)] sm:items-center sm:p-6" role="presentation" onkeydown={handleDialogKeydown}>
    <button class="absolute inset-0 size-full border-0 bg-black/45 backdrop-blur-md" type="button" tabindex="-1" aria-label="Закрыть форму" onclick={closeDialog}></button>
    <dialog open class="relative z-10 m-0 max-h-[calc(100dvh-20px)] w-full overflow-y-auto rounded-t-[30px] border border-b-0 border-[#dedee1] bg-white px-5 pb-[max(1.5rem,env(safe-area-inset-bottom))] pt-6 text-[#09090b] shadow-2xl sm:max-w-xl sm:rounded-[30px] sm:border-b sm:p-6" aria-labelledby="server-form-title">
      <header class="mb-6 flex items-start justify-between"><div><span class="text-xs font-bold tracking-[0.18em] text-[#74747d] uppercase">{dialog === "new" ? "Новый VPS" : "WireGuard"}</span><h2 class="mt-1.5 text-2xl font-bold tracking-[-0.04em]" id="server-form-title">{dialog === "import" ? "Импорт профиля" : dialog === "edit" ? "Изменить профиль" : dialog === "new" ? probe ? "Подтвердите VPS" : "Подключить VPS" : "Профиль для друга"}</h2></div><button class="grid size-12 shrink-0 place-items-center rounded-2xl border border-[#dedee1] bg-white" type="button" disabled={busy} aria-label="Закрыть" onclick={closeDialog}><X size={22} /></button></header>
      {#if dialog === "import"}
        <form class="grid gap-4.5" onsubmit={saveImport}><label><span class="mb-2 block text-xs font-semibold text-[#74747d]">Название</span><input class="h-14 w-full rounded-2xl border border-[#dedee1] px-4" bind:this={nameInput} bind:value={name} required maxlength="40" autocomplete="off" placeholder="Frankfurt" /></label><p class="m-0 text-xs leading-relaxed text-[#74747d]">Вставьте весь WireGuard-профиль. Повторный импорт обновит ключ подключения к этому серверу.</p><label><span class="mb-2 block text-xs font-semibold text-[#74747d]">WireGuard-профиль</span><textarea class="min-h-64 w-full resize-y rounded-2xl border border-[#dedee1] p-4 font-mono text-xs leading-relaxed" bind:value={profile} required maxlength="4096" spellcheck="false" autocomplete="off" placeholder="[Interface] PrivateKey = ..."></textarea></label><button class="min-h-13 rounded-2xl border border-[#09090b] bg-[#09090b] px-4 text-sm font-bold text-white" type="submit" disabled={busy}>{mutation === "import-server" ? "Импортируем…" : "Импортировать"}</button></form>
      {:else if dialog === "edit"}
        <form class="grid gap-4.5" onsubmit={saveEdit}><label><span class="mb-2 block text-xs font-semibold text-[#74747d]">Название</span><input class="h-14 w-full rounded-2xl border border-[#dedee1] px-4" bind:this={nameInput} bind:value={name} required maxlength="40" autocomplete="off" /></label>{#if editing && !editing.managed}<label><span class="mb-2 block text-xs font-semibold text-[#74747d]">Endpoint</span><input class="h-14 w-full rounded-2xl border border-[#dedee1] px-4" bind:value={endpoint} required maxlength="255" /></label><label><span class="mb-2 block text-xs font-semibold text-[#74747d]">Публичный ключ WireGuard</span><input class="h-14 w-full rounded-2xl border border-[#dedee1] px-4" bind:value={publicKey} required maxlength="128" /></label>{:else}<p class="m-0 text-xs leading-relaxed text-[#74747d]">Для управляемого VPS можно изменить только название.</p>{/if}<button class="min-h-13 rounded-2xl border border-[#09090b] bg-[#09090b] px-4 text-sm font-bold text-white" type="submit" disabled={busy}>{mutation?.startsWith("edit:") ? "Сохраняем…" : "Сохранить"}</button></form>
      {:else if dialog === "new"}
        {#if probe}<div class="grid gap-4.5"><p class="m-0 rounded-2xl border border-[#bde4cc] bg-[#edf9f1] p-4 text-sm leading-relaxed text-[#185c38]" role="status"><strong class="block">Ключ SSH получен</strong>Отпечаток SSH: <code class="mt-2 block wrap-break-word text-xs">{probe.fingerprint}</code></p><p class="m-0 text-xs leading-relaxed text-[#74747d]">Сверьте отпечаток с провайдером. После подтверждения Gofro настроит VPS.</p><button class="min-h-13 rounded-2xl border border-[#09090b] bg-[#09090b] px-4 text-sm font-bold text-white" type="button" disabled={busy} onclick={bootstrap}>{mutation === "bootstrap-server" ? "Настраиваем…" : "Подтвердить и настроить"}</button><button class="min-h-13 rounded-2xl border border-[#dedee1] bg-white px-4 text-sm font-bold" type="button" disabled={busy} onclick={() => { probe = null; password = ""; }}>Изменить данные</button></div>
        {:else}<form class="grid gap-4.5" onsubmit={probeServer}><label><span class="mb-2 block text-xs font-semibold text-[#74747d]">Название</span><input class="h-14 w-full rounded-2xl border border-[#dedee1] px-4" bind:this={nameInput} bind:value={name} required maxlength="40" autocomplete="off" placeholder="Frankfurt" /></label><label><span class="mb-2 block text-xs font-semibold text-[#74747d]">IP-адрес VPS</span><input class="h-14 w-full rounded-2xl border border-[#dedee1] px-4" bind:value={host} required maxlength="255" inputmode="text" autocomplete="off" placeholder="2001:db8::1" /></label><label><span class="mb-2 block text-xs font-semibold text-[#74747d]">SSH-порт</span><input class="h-14 w-full rounded-2xl border border-[#dedee1] px-4" bind:value={port} required type="number" min="1" max="65535" inputmode="numeric" /></label><PasswordInput label="Пароль root" bind:value={password} required autocomplete="off" disabled={busy} /><p class="m-0 text-xs leading-relaxed text-[#74747d]">Пароль используется один раз для настройки и не сохраняется.</p><button class="min-h-13 rounded-2xl border border-[#09090b] bg-[#09090b] px-4 text-sm font-bold text-white" type="submit" disabled={busy}>{mutation === "probe-server" ? "Проверяем…" : "Проверить VPS"}</button></form>{/if}
      {:else}<div class="grid gap-4"><textarea class="min-h-64 w-full resize-y rounded-2xl border border-[#dedee1] p-4 font-mono text-xs leading-relaxed" readonly value={friendProfile}></textarea><button class="flex min-h-13 items-center justify-center gap-2 rounded-2xl border border-[#09090b] bg-[#09090b] px-4 text-sm font-bold text-white" type="button" onclick={downloadProfile}><Download size={18} />Скачать .conf</button></div>
      {/if}
      {#if validationError || actionError}<p class="mt-4 rounded-2xl border border-red-200 bg-red-50 p-3 text-xs leading-relaxed text-red-700" role="alert">{validationError || actionError}</p>{/if}
    </dialog>
  </div>
{/if}
