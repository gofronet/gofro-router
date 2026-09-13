<script lang="ts">
  import { onDestroy } from "svelte";
  import ChevronRight from "lucide-svelte/icons/chevron-right";
  import MoreHorizontal from "lucide-svelte/icons/more-horizontal";
  import Plus from "lucide-svelte/icons/plus";

  import { getAppContext } from "../app-context";
  import Dialog from "../components/dialog.svelte";
  import OperationWarning from "../components/operation-warning.svelte";
  import ServerDialogs, { type ServerDialogFlow } from "../components/server-dialogs.svelte";
  import ServerMarker from "../components/server-marker.svelte";
  import { p, route, serverManagementPath } from "../router";
  import { friendNameSchema } from "../api/schemas";
  import type { FriendPeer, ManagedServerStatus, Profile } from "../domain/models";

  type PeerFlow = { kind: "create" } | { kind: "edit"; peer: FriendPeer } | { kind: "revoke"; peer: FriendPeer } | { kind: "share"; peer: FriendPeer } | null;
  type Maintenance = "update" | "restart" | null;

  const app = getAppContext();
  const status = $derived(app.status);
  const busy = $derived(app.busy);
  const serverKey = $derived(typeof route.search.server === "string" ? route.search.server : "");
  const server = $derived(status.servers.find((item) => item.public_key === serverKey));
  const importedAvailability = $derived(server && !server.managed ? server.public_key === status.active_server_key && app.connected ? "Роутер подключён к этому серверу." : "Роутер не подтверждает подключение к этому импортированному профилю." : "");
  let details = $state<ManagedServerStatus | null>(null);
  let inspection = $state("");
  let updateResult = $state("");
  let checkedAt = $state<Date | null>(null);
  let inspectedAt = $state<Date | null>(null);
  let loadingDetails = $state(false);
  let peerFlow = $state<PeerFlow>(null);
  let peerName = $state("");
  let profile = $state<Profile | null>(null);
  let shareMessage = $state("");
  let dialogError = $state("");
  let maintenance = $state<Maintenance>(null);
  let editFlow = $state<ServerDialogFlow>(null);
  let loadedKey = $state("");
  let selectionRevision = 0;

  const peers = $derived(details?.peers.filter(peer => !peer.revoked) ?? []);
  const activePeers = $derived(details ? peers.length : null);

  function characterCount(value: string): number { return Array.from(value.trim()).length; }
  function request(key: string): { key: string; revision: number } { return { key, revision: selectionRevision }; }
  function current(value: { key: string; revision: number }): boolean { return value.revision === selectionRevision && value.key === serverKey; }
  function error(): void { dialogError = app.actionError || "Операция не выполнена."; }
  function committed(): boolean {
    if (!app.actionWarning || app.actionError) return false;
    details = null;
    peerFlow = null;
    maintenance = null;
    return true;
  }
  function openPeer(flow: PeerFlow): void { app.clearActionError(); dialogError = ""; profile = null; shareMessage = ""; peerFlow = flow; }
  function closePeer(): void { if (!busy) { peerFlow = null; profile = null; shareMessage = ""; } }

  async function inspect(key: string, label = "Проверка"): Promise<void> {
    const pending = request(key);
    loadingDetails = true;
    const result = await app.inspectServer(key);
    if (!current(pending)) return;
    loadingDetails = false;
    inspectedAt = new Date();
    if (result) { details = result; inspection = `${label}: сервер ответил, версия ${result.version}.`; }
    else inspection = `${label} не выполнена${app.actionError ? `: ${app.actionError}` : "."}`;
  }

  async function checkUpdates(key: string): Promise<void> {
    app.clearActionError();
    updateResult = "";
    const pending = request(key);
    const result = await app.checkServer(key);
    if (!current(pending)) return;
    checkedAt = new Date();
    updateResult = result ? `Версия ${result.version}. ${result.update_available ? "Доступно обновление." : "Обновлений нет."}` : `Проверка обновлений не выполнена${app.actionError ? `: ${app.actionError}` : "."}`;
  }

  $effect(() => {
    const key = serverKey;
    if (key === loadedKey) return;
    loadedKey = key;
    selectionRevision++;
    details = null;
    inspection = "";
    updateResult = "";
    checkedAt = null;
    inspectedAt = null;
    loadingDetails = false;
    peerFlow = null;
    profile = null;
    shareMessage = "";
    dialogError = "";
    maintenance = null;
    editFlow = null;
    if (key && server?.managed) void inspect(key, "Первичная проверка");
  });
  onDestroy(() => { selectionRevision++; });

  async function savePeer(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (!server || !peerFlow || busy || (peerFlow.kind !== "create" && peerFlow.kind !== "edit")) return;
    const parsed = friendNameSchema.safeParse(peerName);
    if (!parsed.success) { dialogError = "Введите имя от 1 до 60 символов без управляющих знаков."; return; }
    const name = parsed.data;
    const editedKey = peerFlow.kind === "edit" ? peerFlow.peer.public_key : null;
    if (peers.some(peer => peer.public_key !== editedKey && peer.name.toLowerCase() === name.toLowerCase())) {
      dialogError = "Активный доступ с таким именем уже существует.";
      return;
    }
    dialogError = "";
    const pending = request(server.public_key);
    const result = peerFlow.kind === "edit" ? await app.renameFriend(pending.key, peerFlow.peer.public_key, name) : await app.createFriend(pending.key, name);
    if (!current(pending)) return;
    if (!result && committed()) return;
    if (!result) return error();
    details = result;
    closePeer();
  }

  async function revokePeer(): Promise<void> {
    if (!server || peerFlow?.kind !== "revoke" || busy) return;
    const pending = request(server.public_key);
    const result = await app.revokeFriend(pending.key, peerFlow.peer.public_key);
    if (!current(pending)) return;
    if (!result && committed()) return;
    if (!result) return error();
    details = result;
    closePeer();
  }

  async function openShare(peer: FriendPeer): Promise<void> {
    if (!server || busy) return;
    openPeer({ kind: "share", peer });
    const pending = request(server.public_key);
    const result = await app.friendProfile(pending.key, peer.public_key);
    if (!current(pending) || peerFlow?.kind !== "share" || peerFlow.peer.public_key !== peer.public_key) return;
    if (!result) return error();
    profile = result;
  }

  async function copyProfile(): Promise<void> {
    if (!profile) return;
    const pending = request(serverKey);
    try { await navigator.clipboard.writeText(profile.profile); if (current(pending)) shareMessage = "Конфигурация скопирована."; }
    catch { if (current(pending)) { document.querySelector<HTMLTextAreaElement>("#peer-profile")?.select(); shareMessage = "Выделите конфигурацию и скопируйте вручную."; } }
  }

  function downloadProfile(): void {
    if (!profile) return;
    try {
      const url = URL.createObjectURL(new Blob([profile.profile], { type: "text/plain;charset=utf-8" }));
      const link = document.createElement("a");
      link.href = url;
      link.download = "wg.conf";
      document.body.append(link);
      link.click();
      link.remove();
      window.setTimeout(() => URL.revokeObjectURL(url), 1_000);
    } catch { document.querySelector<HTMLTextAreaElement>("#peer-profile")?.select(); shareMessage = "Выделите конфигурацию и сохраните вручную."; }
  }

  async function maintain(): Promise<void> {
    if (!server || !maintenance || busy) return;
    const action = maintenance;
    const pending = request(server.public_key);
    const result = action === "update" ? await app.updateManagedServer(pending.key) : await app.restartServer(pending.key);
    if (!current(pending)) return;
    if (!result && committed()) return;
    if (!result) return error();
    maintenance = null;
    await inspect(pending.key, action === "update" ? "Обновление завершено" : "VPN перезапущен");
  }
</script>

<svelte:head><title>Управление сервером · Gofro Router</title></svelte:head>

{#if app.warningServerKey === serverKey}<OperationWarning onrefresh={() => inspect(serverKey)} />{/if}

{#if !serverKey}
  <section class="panel"><div class="panel-head"><h2>Выберите сервер</h2></div><div class="panel-body">{#each status.servers as item (item.public_key)}<a class="choice-button" href={serverManagementPath(item.public_key)}><ServerMarker emoji={item.emoji} /><span><strong>{item.name}</strong><small>{item.managed ? "Свой VPS" : "VPN-профиль"}</small></span><ChevronRight class="icon" /></a>{:else}<p class="small">Сначала добавьте свой VPS, чтобы управлять доступами друзей.</p>{/each}<div class="form-actions"><button class="btn primary" type="button" disabled={busy} onclick={() => editFlow = { kind: "vps" }}><Plus size={17} />Добавить свой VPS</button></div></div></section>
{:else if !server}
  <div class="management"><a class="text-link management-back" href={p("/servers")}>Все серверы</a><section class="panel empty"><h2>Сервер не найден</h2><p>Он удалён или ссылка устарела.</p></section></div>
{:else}
  <div class="management"><a class="text-link management-back" href={p("/servers")}>Все серверы</a><div class="section-caption management-heading"><div><h2>{server.name}</h2><p>{server.managed ? "Свой VPS" : "Импортированный профиль"} · <span class="mono">{server.endpoint}</span></p></div><button class="btn" type="button" disabled={busy} onclick={() => editFlow = { kind: "edit", publicKey: server.public_key }}>Настройки</button></div>
    <section class="panel"><div class="panel-head"><h2>Доступность</h2><button class="btn" type="button" disabled={busy || !server.managed} onclick={() => inspect(server.public_key)}>Проверить</button></div><div class="panel-body">{#if inspectedAt}<p class="small muted">Последняя попытка: {inspectedAt.toLocaleString("ru-RU")}</p>{/if}<p class="management-result" role="status">{server.managed ? inspection || (loadingDetails ? "Проверяем сервер…" : "Ещё не проверено") : importedAvailability}</p></div></section>
    {#if server.managed}
      <section class="panel"><div class="panel-head"><h2>Доступы друзей {#if activePeers !== null}<span class="count">{activePeers}</span>{/if}</h2><button class="btn primary" type="button" disabled={busy || !details} onclick={() => { peerName = ""; openPeer({ kind: "create" }); }}>Создать доступ</button></div>
        {#if details}{#each peers as peer (peer.public_key)}<article class="full-row peer-row"><div class="row-main"><h3>{peer.name}</h3><p>Доступ активен</p></div><div class="row-actions">{#if peer.can_share}<button class="btn" type="button" disabled={busy} onclick={() => openShare(peer)}>Поделиться</button>{:else}<span class="small muted">Профиль для этого старого доступа не хранится, поэтому поделиться им нельзя.</span>{/if}<button class="icon-btn" type="button" disabled={busy} aria-label={`Изменить доступ ${peer.name}`} onclick={() => { peerName = peer.name; openPeer({ kind: "edit", peer }); }}><MoreHorizontal size={19} /></button></div></article>{:else}<div class="empty"><h3>Пока нет доступов</h3><p>Для каждого друга создайте отдельный доступ.</p></div>{/each}{:else}<div class="empty"><h3>{loadingDetails ? "Получаем доступы" : "Доступы недоступны"}</h3><p>{loadingDetails ? "Запрашиваем данные сервера." : "Повторите проверку сервера, чтобы загрузить доступы."}</p>{#if !loadingDetails}<button class="btn" type="button" disabled={busy} onclick={() => inspect(server.public_key)}>Повторить</button>{/if}</div>{/if}
      </section>
      <section class="panel"><details class="disclosure"><summary>Обслуживание</summary><div class="details-content"><div class="management-actions"><button class="btn" type="button" disabled={busy} onclick={() => checkUpdates(server.public_key)}>Проверить обновления</button><button class="btn" type="button" disabled={busy} onclick={() => { app.clearActionError(); dialogError = ""; maintenance = "update"; }}>Обновить</button><button class="btn" type="button" disabled={busy} onclick={() => { app.clearActionError(); dialogError = ""; maintenance = "restart"; }}>Перезапустить VPN</button></div>{#if checkedAt}<p class="management-result" role="status">Последняя проверка: {checkedAt.toLocaleString("ru-RU")}. {updateResult}</p>{/if}</div></details></section>
    {:else}<p class="support-text">Доступами и обновлениями управляет владелец сервера.</p>{/if}
  </div>
{/if}

{#if peerFlow?.kind === "create"}
  <Dialog title="Доступ другу" onclose={closePeer} {busy}><form onsubmit={savePeer}><label class="field">Имя друга<input bind:value={peerName} required maxlength="120" autocomplete="off" /></label>{#if characterCount(peerName) > 60}<p class="error" role="alert">Название не должно быть длиннее 60 символов.</p>{/if}{#if dialogError || app.actionError}<p class="error" role="alert">{dialogError || app.actionError}</p>{/if}<div class="form-actions"><button class="btn primary" type="submit" disabled={busy || characterCount(peerName) > 60}>Создать доступ</button></div></form></Dialog>
{:else if peerFlow?.kind === "edit"}
  {@const peer = peerFlow.peer}
  <Dialog title="Изменить доступ" onclose={closePeer} {busy}><form onsubmit={savePeer}><label class="field">Имя друга<input bind:value={peerName} required maxlength="120" autocomplete="off" /></label>{#if characterCount(peerName) > 60}<p class="error" role="alert">Название не должно быть длиннее 60 символов.</p>{/if}{#if dialogError || app.actionError}<p class="error" role="alert">{dialogError || app.actionError}</p>{/if}<div class="form-actions"><button class="btn ghost danger" type="button" disabled={busy} onclick={() => openPeer({ kind: "revoke", peer })}>Отозвать</button><button class="btn primary" type="submit" disabled={busy || characterCount(peerName) > 60}>Сохранить</button></div></form></Dialog>
{:else if peerFlow?.kind === "revoke"}
  {@const peer = peerFlow.peer}
  <Dialog title="Отозвать доступ?" onclose={closePeer} {busy}><p class="dialog-intro">{peer.name} больше не сможет пользоваться этим доступом. Ваше подключение, другие доступы и роутер не изменятся.</p>{#if dialogError || app.actionError}<p class="error" role="alert">{dialogError || app.actionError}</p>{/if}<div class="form-actions"><button class="btn ghost" type="button" disabled={busy} onclick={() => peerFlow = { kind: "edit", peer }}>Отмена</button><button class="btn danger" type="button" disabled={busy} onclick={revokePeer}>Отозвать</button></div></Dialog>
{:else if peerFlow?.kind === "share"}
  {@const peer = peerFlow.peer}
  <Dialog title="Поделиться доступом" onclose={closePeer} {busy}><p class="dialog-intro">Конфигурация для {peer.name}.</p>{#if profile}<label class="field">Конфигурация<textarea id="peer-profile" readonly value={profile.profile} spellcheck="false"></textarea></label><p class="small">Импорт у друга: VPN → Добавить → Файл настроек VPN.</p><div class="form-actions peer-share-actions"><button class="btn" type="button" onclick={copyProfile}>Копировать текст</button><button class="btn primary" type="button" onclick={downloadProfile}>Скачать wg.conf</button></div><p class="management-result" role="status">{shareMessage}</p>{:else if busy}<p class="dialog-intro">Получаем конфигурацию доступа…</p>{:else}<p class="error" role="alert">{dialogError || app.actionError || "Конфигурация недоступна."}</p><button class="btn" type="button" onclick={() => openShare(peer)}>Повторить</button>{/if}</Dialog>
{/if}
{#if maintenance}<Dialog title={maintenance === "update" ? "Обновить VPN на сервере?" : "Перезапустить VPN на сервере?"} onclose={() => { if (!busy) maintenance = null; }} {busy}><p class="dialog-intro">Подключение прервётся у вас и у всех друзей.</p>{#if dialogError || app.actionError}<p class="error" role="alert">{dialogError || app.actionError}</p>{/if}<div class="form-actions"><button class="btn ghost" type="button" disabled={busy} onclick={() => maintenance = null}>Отмена</button><button class="btn primary" type="button" disabled={busy} onclick={maintain}>{maintenance === "update" ? "Обновить" : "Перезапустить"}</button></div></Dialog>{/if}
<ServerDialogs bind:flow={editFlow} />
