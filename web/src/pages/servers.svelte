<script lang="ts">
  import { tick } from "svelte";
  import Check from "lucide-svelte/icons/check";
  import Pencil from "lucide-svelte/icons/pencil";
  import Plus from "lucide-svelte/icons/plus";
  import ServerIcon from "lucide-svelte/icons/server";
  import Trash2 from "lucide-svelte/icons/trash-2";
  import X from "lucide-svelte/icons/x";

  import type { Server } from "../domain/models";
  import { getAppContext } from "../app-context";
  import { shortKey } from "../format";

  const app = getAppContext();
  const status = $derived(app.status);
  const busy = $derived(app.busy);
  const mutation = $derived(app.mutation);

  let editing = $state<Server | null | undefined>(undefined);
  let name = $state("");
  let endpoint = $state("");
  let publicKey = $state("");
  let profile = $state("");
  let validationError = $state("");
  let nameInput = $state<HTMLInputElement>();

  const activeServer = $derived(
    status.servers.find(
      (server) => server.public_key === status.active_server_key,
    ),
  );

  async function openForm(server: Server | null) {
    editing = server;
    name = server?.name || "";
    endpoint = server?.endpoint || "";
    publicKey = server?.public_key || "";
    profile = "";
    validationError = "";
    await tick();
    nameInput?.focus();
  }

  function closeForm() {
    if (!busy) {
      profile = "";
      editing = undefined;
    }
  }

  async function save(event: SubmitEvent) {
    event.preventDefault();
    if (!editing) {
      const values = { name: name.trim(), profile: profile.trim() };
      if (!values.name || !values.profile) {
        validationError = "Заполните название и вставьте WireGuard-профиль.";
        return;
      }
      validationError = "";
      if (await app.importServer(values)) {
        profile = "";
        editing = undefined;
      }
      return;
    }
    const values = {
      name: name.trim(),
      endpoint: endpoint.trim(),
      public_key: publicKey.trim(),
    };
    if (!values.name || !values.endpoint || !values.public_key) {
      validationError = "Заполните название, endpoint и публичный ключ.";
      return;
    }
    validationError = "";
    const updated = await app.updateServer(editing.public_key, values);
    if (updated) editing = undefined;
  }

  async function selectServer(server: Server) {
    if (server.public_key === status.active_server_key) return;
    await app.selectServer(server.public_key);
  }

  async function deleteServer(server: Server) {
    if (!confirm(`Удалить профиль «${server.name}»?`)) return;
    await app.removeServer(server.public_key);
  }

  function handleDialogKeydown(event: KeyboardEvent) {
    if (event.key === "Escape") closeForm();
  }
</script>

<svelte:head><title>Серверы · Gofro Router</title></svelte:head>

<section class="grid min-w-0 gap-5 lg:gap-6" aria-labelledby="servers-title">
  <header
    class="min-w-0 px-0.5 py-2 sm:flex sm:items-end sm:justify-between sm:gap-6"
  >
    <div class="min-w-0">
      <span class="text-xs font-bold tracking-[0.18em] text-[#74747d] uppercase"
        >VPN-профили</span
      >
      <h1
        class="mt-2 text-[clamp(2.25rem,11vw,3.25rem)] leading-[0.98] font-extrabold tracking-[-0.06em] lg:text-[clamp(3rem,5vw,4.2rem)]"
        id="servers-title"
      >
        Конфигурации
      </h1>
      <p class="mt-3.5 max-w-2xl text-base leading-relaxed text-[#74747d]">
        Выберите маршрут или добавьте новый WireGuard-сервер.
      </p>
    </div>
    <button
      class="mt-4 flex min-h-13 shrink-0 items-center justify-center gap-2 rounded-2xl border border-[#09090b] bg-[#09090b] px-5 text-sm font-bold text-white sm:mb-1 sm:mt-0"
      type="button"
      onclick={() => openForm(null)}><Plus size={19} />Добавить</button
    >
  </header>

  <article
    class="grid min-h-48 min-w-0 grid-cols-[3.125rem_minmax(0,1fr)] items-center gap-4 overflow-hidden rounded-[28px] bg-[linear-gradient(145deg,#202024,#09090b_72%)] p-6 text-white shadow-xl shadow-black/10 lg:grid-cols-[3.125rem_minmax(0,1fr)_auto] lg:p-7"
  >
    <div
      class="grid size-12.5 place-items-center rounded-2xl bg-white text-[#09090b]"
    >
      <ServerIcon size={22} />
    </div>
    <div class="min-w-0">
      <span class="text-xs text-[#aaaab1]">Активный профиль</span>
      <h2 class="my-1.5 text-2xl font-bold tracking-[-0.045em]">
        {activeServer?.name || "Сервер не выбран"}
      </h2>
      <p
        class="m-0 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[0.7rem] text-[#aaaab1]"
      >
        {activeServer?.endpoint || "Выберите профиль из списка ниже."}
      </p>
    </div>
    <span
      class={`col-span-2 flex min-h-12 items-center gap-2 border-t border-[#343438] pt-3 text-xs font-bold lg:col-span-1 lg:min-w-40 lg:border-0 lg:pt-0 ${status.tunnel_active ? "text-white" : "text-red-300"}`}
      ><i class="size-2 bg-current"></i>{status.tunnel_active
        ? "Туннель активен"
        : "Нет соединения"}</span
    >
  </article>

  <section class="mt-2 grid min-w-0 gap-3" aria-labelledby="profiles-title">
    <header class="flex min-w-0 items-end justify-between gap-3 px-1">
      <div class="min-w-0">
        <span
          class="text-xs font-bold tracking-[0.18em] text-[#74747d] uppercase"
          >Сохраненные маршруты</span
        >
        <h2
          class="mt-1.5 text-xl font-bold tracking-[-0.035em]"
          id="profiles-title"
        >
          Профили серверов
        </h2>
      </div>
      <span class="shrink-0 text-xs font-semibold text-[#74747d]"
        >{status.servers.length} всего</span
      >
    </header>

    {#if status.servers.length === 0}
      <div
        class="flex min-h-52 flex-col items-center justify-center gap-2 rounded-[28px] border border-[#dedee1] bg-white p-8 text-center text-[#74747d] shadow-sm"
      >
        <ServerIcon size={28} /><strong class="mt-1 text-[#09090b]"
          >Профилей пока нет</strong
        ><span class="max-w-sm text-sm leading-relaxed"
          >Добавьте первый VPN-сервер, чтобы включить защищенный маршрут.</span
        ><button
          class="mt-3 flex min-h-12 items-center gap-2 rounded-2xl border border-[#dedee1] bg-white px-5 text-sm font-bold"
          type="button"
          onclick={() => openForm(null)}
          ><Plus size={18} />Добавить профиль</button
        >
      </div>
    {:else}
      <div class="grid min-w-0 gap-3 sm:grid-cols-2 xl:grid-cols-3">
        {#each status.servers as server (server.public_key)}
          {@const active = server.public_key === status.active_server_key}
          <article
            class={`min-w-0 overflow-hidden rounded-[28px] border bg-white shadow-sm ${active ? "border-[#09090b] ring-1 ring-[#09090b]" : "border-[#dedee1]"}`}
          >
            <button
              class="grid min-h-32 w-full min-w-0 grid-cols-[1.375rem_minmax(0,1fr)] items-start gap-3 border-0 bg-transparent px-4 py-5 text-left min-[420px]:grid-cols-[1.375rem_minmax(0,1fr)_auto]"
              type="button"
              aria-pressed={active}
              disabled={busy || active}
              onclick={() => selectServer(server)}
            >
              <span
                class={`grid size-5.5 place-items-center rounded-[7px] border ${active ? "border-[#09090b] bg-[#09090b] text-white" : "border-[#aaaab0]"}`}
                >{#if active}<Check size={16} />{/if}</span
              >
              <span class="min-w-0">
                <strong
                  class="block overflow-hidden text-ellipsis whitespace-nowrap text-sm"
                  >{server.name}</strong
                >
                <small
                  class="mt-1.5 block overflow-hidden text-ellipsis whitespace-nowrap text-xs text-[#74747d]"
                  >{server.endpoint}</small
                >
                <code
                  class="mt-2.5 block overflow-hidden text-ellipsis whitespace-nowrap text-[0.62rem] text-[#a0a0a7]"
                  title={server.public_key}>{shortKey(server.public_key)}</code
                >
              </span>
              <span
                class="col-start-2 mt-1 justify-self-start text-[0.68rem] font-bold min-[420px]:col-auto min-[420px]:mt-0 min-[420px]:self-center"
                >{mutation === `select:${server.public_key}`
                  ? "Подключаем…"
                  : active
                    ? "Активен"
                    : "Выбрать"}</span
              >
            </button>
            <div
              class="grid grid-cols-2 divide-x divide-[#ececef] border-t border-[#ececef]"
            >
              <button
                class="flex min-h-13 items-center justify-center gap-2 border-0 bg-transparent text-xs font-semibold text-[#74747d]"
                type="button"
                disabled={busy}
                onclick={() => openForm(server)}
                aria-label={`Изменить профиль ${server.name}`}
                ><Pencil size={18} /><span>Изменить</span></button
              >
              <button
                class="flex min-h-13 items-center justify-center gap-2 border-0 bg-transparent text-xs font-semibold text-red-700"
                type="button"
                disabled={busy}
                onclick={() => deleteServer(server)}
                aria-label={`Удалить профиль ${server.name}`}
                ><Trash2 size={18} /><span
                  >{mutation === `delete:${server.public_key}`
                    ? "Удаляем…"
                    : "Удалить"}</span
                ></button
              >
            </div>
          </article>
        {/each}
      </div>
    {/if}
  </section>
</section>

{#if editing !== undefined}
  <div
    class="fixed inset-0 z-100 flex items-end justify-center pt-[env(safe-area-inset-top)] sm:items-center sm:p-6"
    role="presentation"
    onkeydown={handleDialogKeydown}
  >
    <button
      class="absolute inset-0 size-full border-0 bg-black/45 backdrop-blur-md"
      type="button"
      tabindex="-1"
      aria-label="Закрыть форму"
      onclick={closeForm}
    ></button>
    <dialog
      open
      class="relative z-10 m-0 max-h-[calc(100dvh-20px)] w-full overflow-y-auto rounded-t-[30px] border border-b-0 border-[#dedee1] bg-white px-5 pb-[max(1.5rem,env(safe-area-inset-bottom))] pt-6 text-[#09090b] shadow-2xl sm:max-w-xl sm:rounded-[30px] sm:border-b sm:p-6"
      aria-labelledby="profile-form-title"
    >
      <header class="mb-6 flex items-start justify-between">
        <div>
          <span
            class="text-xs font-bold tracking-[0.18em] text-[#74747d] uppercase"
            >WireGuard</span
          >
          <h2
            class="mt-1.5 text-2xl font-bold tracking-[-0.04em]"
            id="profile-form-title"
          >
            {editing ? "Изменить профиль" : "Импорт профиля"}
          </h2>
        </div>
        <button
          class="grid size-12 shrink-0 place-items-center rounded-2xl border border-[#dedee1] bg-white"
          type="button"
          disabled={busy}
          aria-label="Закрыть"
          onclick={closeForm}><X size={22} /></button
        >
      </header>
      <form class="grid gap-4.5" onsubmit={save}>
        <label>
          <span class="mb-2 block text-xs font-semibold text-[#74747d]"
            >Название</span
          >
          <input
            class="h-14 w-full rounded-2xl border border-[#dedee1] bg-white px-4 text-base"
            bind:this={nameInput}
            bind:value={name}
            required
            maxlength="40"
            autocomplete="off"
            placeholder="Frankfurt"
          />
        </label>
        {#if editing}
          <label>
            <span class="mb-2 block text-xs font-semibold text-[#74747d]"
              >Endpoint</span
            >
            <input
              class="h-14 w-full rounded-2xl border border-[#dedee1] bg-white px-4 text-base"
              bind:value={endpoint}
              required
              maxlength="255"
              spellcheck="false"
              autocomplete="off"
              placeholder="203.0.113.10:8443"
            />
          </label>
          <label>
            <span class="mb-2 block text-xs font-semibold text-[#74747d]"
              >Публичный ключ WireGuard</span
            >
            <input
              class="h-14 w-full rounded-2xl border border-[#dedee1] bg-white px-4 text-base"
              bind:value={publicKey}
              required
              maxlength="128"
              spellcheck="false"
              autocomplete="off"
              placeholder="Base64 public key"
            />
          </label>
        {:else}
          <p class="m-0 text-xs leading-relaxed text-[#74747d]">
            На VPS выполните <code
              >sudo gofro-router-server create-profile --endpoint &lt;VPS-IP&gt;:8443</code
            > и вставьте весь полученный профиль. Повторный импорт обновит ключ
            подключения к этому серверу.
          </p>
          <label>
            <span class="mb-2 block text-xs font-semibold text-[#74747d]"
              >WireGuard-профиль</span
            >
            <textarea
              class="min-h-64 w-full resize-y rounded-2xl border border-[#dedee1] bg-white p-4 font-mono text-xs leading-relaxed"
              bind:value={profile}
              required
              maxlength="4096"
              spellcheck="false"
              autocomplete="off"
              placeholder={'[Interface]\nPrivateKey = …\nAddress = 10.202.0.2/32\n\n[Peer]\nPublicKey = …\nEndpoint = 203.0.113.10:8443'}
            ></textarea>
          </label>
        {/if}
        {#if validationError}<p
            class="m-0 rounded-2xl border border-red-200 bg-red-50 p-3 text-xs leading-relaxed text-red-700"
            role="alert"
          >
            {validationError}
          </p>{/if}
        <div class="mt-1 grid grid-cols-[0.8fr_1.2fr] gap-2.5">
          <button
            class="min-h-13 rounded-2xl border border-[#dedee1] bg-white px-4 text-sm font-bold"
            type="button"
            disabled={busy}
            onclick={closeForm}>Отмена</button
          >
          <button
            class="min-h-13 rounded-2xl border border-[#09090b] bg-[#09090b] px-4 text-sm font-bold text-white"
            type="submit"
            disabled={busy}
            >{mutation?.startsWith("edit:") || mutation === "import-server"
              ? "Сохраняем…"
              : editing
                ? "Сохранить"
                : "Импортировать"}</button
          >
        </div>
      </form>
    </dialog>
  </div>
{/if}
