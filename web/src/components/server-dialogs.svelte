<script module lang="ts">
  export type ServerDialogFlow =
    | { kind: "choose" | "add" | "import" | "vps" | "disconnect" }
    | { kind: "edit" | "connect" | "delete"; publicKey: string }
    | null;
</script>

<script lang="ts">
  import Check from "lucide-svelte/icons/check";
  import ServerIcon from "lucide-svelte/icons/server";
  import Upload from "lucide-svelte/icons/upload";

  import { getAppContext } from "../app-context";
  import Dialog from "./dialog.svelte";
  import PasswordInput from "./password-input.svelte";
  import type { Server, ServerProbe } from "../domain/models";
  import { p as path, serverManagementPath } from "../router";
  import ServerMarker from "./server-marker.svelte";

  let { flow = $bindable<ServerDialogFlow>(null), onadded }: { flow?: ServerDialogFlow; onadded?: () => Promise<void> } = $props();

  const app = getAppContext();
  const status = $derived(app.status);
  const busy = $derived(app.busy);
  let name = $state("");
  let profile = $state("");
  let endpoint = $state("");
  let publicKey = $state("");
  let host = $state("");
  let port = $state("22");
  let password = $state("");
  let probe = $state<ServerProbe | null>(null);
  let error = $state("");
  let fileName = $state("Файл не выбран");
  let fileVersion = 0;
  let emoji = $state("");
  let lastFlow = "";
  const fileId = $props.id();

  function flowServer(next: ServerDialogFlow): Server | undefined {
    if (!next || !("publicKey" in next)) return undefined;
    return status.servers.find((item) => item.public_key === next.publicKey);
  }

  const server = $derived(flowServer(flow));
  const flowKey = $derived(flow ? `${flow.kind}:${"publicKey" in flow ? flow.publicKey : ""}` : "");

  $effect(() => {
    if (flowKey === lastFlow) return;
    lastFlow = flowKey;
    if (flow) app.clearActionError();
    error = "";
    probe = null;
    fileName = "Файл не выбран";
    fileVersion++;
    if (flow?.kind === "edit" && server) {
      name = server.name;
      endpoint = server.endpoint;
      publicKey = server.public_key;
      emoji = server.emoji || "";
      return;
    }
    name = "";
    profile = "";
    endpoint = "";
    publicKey = "";
    host = "";
    port = "22";
    password = "";
    emoji = "";
  });

  function close() {
    if (busy) return;
    fileVersion++;
    password = "";
    profile = "";
    flow = null;
  }

  function showError(message: string) {
    error = message;
  }

  async function readFile(event: Event & { currentTarget: HTMLInputElement }) {
    const file = event.currentTarget.files?.[0];
    const version = ++fileVersion;
    fileName = file?.name || "Файл не выбран";
    profile = "";
    if (!file) return;
    if (file.size > 4096) {
      showError("Файл WireGuard не должен быть больше 4096 байт.");
      return;
    }
    try {
      const text = await file.text();
      if (version !== fileVersion) return;
      profile = text;
      error = "";
    } catch {
      if (version === fileVersion) showError("Не удалось прочитать выбранный файл.");
    }
  }

  async function importProfile(event: SubmitEvent) {
    event.preventDefault();
    if (busy) return;
    const trimmedName = name.trim();
    if (!trimmedName || !profile.trim()) return showError("Введите название и добавьте WireGuard-профиль.");
    if (Array.from(trimmedName).length > 60) return showError("Название не должно быть длиннее 60 символов.");
    if (new TextEncoder().encode(profile).length > 4096) return showError("WireGuard-профиль не должен быть больше 4096 байт.");
    error = "";
    if (await app.importServer({ name: trimmedName, profile: profile.trim() })) {
      profile = "";
      await onadded?.();
      close();
    }
  }

  async function probeVps(event: SubmitEvent) {
    event.preventDefault();
    if (busy) return;
    const sshPort = Number(port);
    if (!name.trim() || !host.trim() || !password || !Number.isInteger(sshPort) || sshPort < 1 || sshPort > 65535) return showError("Введите название, IP-адрес, SSH-порт и пароль root.");
    if (Array.from(name.trim()).length > 60) return showError("Название не должно быть длиннее 60 символов.");
    error = "";
    probe = await app.probeServer(host.trim(), sshPort);
    if (!probe) password = "";
  }

  async function bootstrap() {
    if (!probe || busy) return;
    if (!password) {
      probe = null;
      return showError("Введите пароль root заново и повторите проверку VPS.");
    }
    const completed = await app.bootstrapServer(name.trim(), probe.host, probe.port, password, probe.host_key);
    password = "";
    if (completed) {
      await onadded?.();
      close();
    } else {
      probe = null;
    }
  }

  async function connect() {
    if (!server || busy) return;
    if (server.public_key !== status.active_server_key && !await app.selectServer(server.public_key)) return;
    if (await app.setMode(true)) close();
  }

  async function disconnect() {
    if (busy) return;
    if (await app.setMode(false)) close();
  }

  async function saveEdit(event: SubmitEvent) {
    event.preventDefault();
    if (!server || busy || !name.trim()) return showError("Введите название профиля.");
    if (Array.from(name.trim()).length > 60) return showError("Название не должно быть длиннее 60 символов.");
    const next = { name: name.trim(), endpoint: server.managed ? server.endpoint : endpoint.trim(), public_key: server.managed ? server.public_key : publicKey.trim(), emoji };
    if (!next.endpoint || !next.public_key) return showError("Введите endpoint и публичный ключ.");
    error = "";
    if (await app.updateServer(server.public_key, next)) close();
  }

  async function remove() {
    if (!server || busy) return;
    if (await app.removeServer(server.public_key)) close();
  }

  const emojiGroups = [
    { title: "Значки", options: [["", "Без значка"], ["🌍", "Земля"], ["🏠", "Дом"], ["💼", "Работа"], ["🚀", "Ракета"], ["⚡", "Молния"], ["🛡️", "Щит"], ["🔑", "Ключ"], ["🐈", "Кот"], ["🦊", "Лиса"], ["🐻", "Медведь"], ["🛰️", "Спутник"]] },
    { title: "Флаги", options: [["🇩🇪", "Германия"], ["🇳🇱", "Нидерланды"], ["🇫🇮", "Финляндия"], ["🇸🇪", "Швеция"], ["🇨🇭", "Швейцария"], ["🇫🇷", "Франция"], ["🇬🇧", "Великобритания"], ["🇺🇸", "США"], ["🇨🇦", "Канада"], ["🇯🇵", "Япония"], ["🇸🇬", "Сингапур"], ["🇹🇷", "Турция"]] },
  ];
  const emojiOptions = $derived(emojiGroups.flatMap((group) => group.options.map(([value]) => value)));
</script>

{#if flow}
  <Dialog title={flow.kind === "choose" ? "Выбрать сервер" : flow.kind === "add" ? "Добавить сервер" : flow.kind === "import" ? "Импорт VPN" : flow.kind === "vps" ? probe ? "Проверьте VPS" : "Свой сервер" : flow.kind === "edit" ? "Настройки сервера" : flow.kind === "connect" ? "Подключить VPN?" : flow.kind === "delete" ? "Удалить сервер?" : "Отключить VPN?"} onclose={close} {busy}>
    {#if flow.kind === "choose"}
      {#each status.servers as item (item.public_key)}
        <button class="choice-button" type="button" disabled={busy || (item.public_key === status.active_server_key && app.connected)} onclick={() => flow = { kind: "connect", publicKey: item.public_key }}><ServerMarker emoji={item.emoji} /><span><strong>{item.name}</strong><small>{item.endpoint}</small></span>{#if item.public_key === status.active_server_key}<Check size={20} />{/if}</button>
      {:else}
        <p class="dialog-intro">Нет настроенных серверов.</p>
      {/each}
      <div class="form-actions"><a class="btn" href={path("/servers")} onclick={close}>Все серверы</a><button class="btn primary" type="button" disabled={busy} onclick={() => flow = { kind: "add" }}>Добавить сервер</button></div>
    {:else if flow.kind === "add"}
      <button class="choice-button" type="button" onclick={() => flow = { kind: "import" }}><Upload size={20} /><span><strong>Файл настроек VPN</strong><small>WireGuard (.conf)</small></span></button>
      <button class="choice-button" type="button" onclick={() => flow = { kind: "vps" }}><ServerIcon size={20} /><span><strong>Свой сервер</strong><small>Настройка через SSH</small></span></button>
    {:else if flow.kind === "import"}
      <form onsubmit={importProfile}>
        <label class="field">Название<input bind:value={name} required maxlength="120" autocomplete="off" /></label>
        <span class="file-field">Файл WireGuard (.conf)<span class="file-upload"><input id={fileId} type="file" accept=".conf,text/plain" aria-label="Выберите файл WireGuard" onchange={readFile} /><label class="btn file-upload-button" for={fileId}><Upload size={17} />Выбрать файл</label><span class="file-upload-name">{fileName}</span></span></span>
        <label class="field">Или вставьте настройки<textarea bind:value={profile} maxlength="4096" spellcheck="false" placeholder="[Interface]"></textarea></label>
        <p class="notice">Ключи передаются роутеру по защищённому соединению и хранятся там с правами 0600.</p>
        <div class="form-actions"><button class="btn primary" type="submit" disabled={busy}>Импортировать</button></div>
      </form>
    {:else if flow.kind === "vps"}
      {#if probe}
        <p class="notice">Проверьте отпечаток SSH для <span class="mono">{probe.host}:{probe.port}</span>: <span class="mono">{probe.fingerprint}</span></p>
        <p class="dialog-intro">После подтверждения Gofro настроит VPS с этим ключом хоста.</p>
        <div class="form-actions"><button class="btn ghost" type="button" disabled={busy} onclick={() => { probe = null; password = ""; }}>Изменить данные</button><button class="btn primary" type="button" disabled={busy} onclick={bootstrap}>Подтвердить и настроить</button></div>
      {:else}
        <form onsubmit={probeVps}>
          <label class="field">Название<input bind:value={name} required maxlength="120" autocomplete="off" /></label>
          <label class="field">IP-адрес VPS<input bind:value={host} required maxlength="255" inputmode="text" autocomplete="off" /></label>
          <div class="form-line"><label class="field">Логин<input value="root" readonly /></label><label class="field">Порт SSH<input bind:value={port} type="number" min="1" max="65535" /></label></div>
          <PasswordInput label="Пароль root" bind:value={password} required autocomplete="off" disabled={busy} />
          <p class="field-help">Пароль используется только для настройки и не сохраняется.</p>
          <div class="form-actions"><button class="btn primary" type="submit" disabled={busy}>Проверить VPS</button></div>
        </form>
      {/if}
    {:else if flow.kind === "edit" && server}
      <form onsubmit={saveEdit}>
        <label class="field">Название<input bind:value={name} required maxlength="120" autocomplete="off" /></label>
        <fieldset class="emoji-picker"><legend>Значок сервера</legend>
          {#if emoji && !emojiOptions.includes(emoji)}<div class="emoji-group-title">Текущий значок</div><div class="emoji-grid"><label class="emoji-option" title="Текущий значок"><input type="radio" name="emoji" bind:group={emoji} value={emoji} aria-label="Текущий значок" /><span aria-hidden="true">{emoji}</span></label></div>{/if}
          {#each emojiGroups as group (group.title)}<div class="emoji-group-title">{group.title}</div><div class="emoji-grid">{#each group.options as option (option[0])}<label class="emoji-option" title={option[1]}><input type="radio" name="emoji" bind:group={emoji} value={option[0]} aria-label={option[1]} /><span aria-hidden="true">{option[0] || "−"}</span></label>{/each}</div>{/each}
        </fieldset>
        {#if !server.managed}<label class="field">Endpoint<input bind:value={endpoint} required maxlength="255" /></label><label class="field">Публичный ключ WireGuard<input bind:value={publicKey} required maxlength="128" /></label>{/if}
        <dl class="key-values"><div><dt>Адрес сервера</dt><dd class="mono">{server.endpoint}</dd></div><div><dt>Состояние</dt><dd>{server.public_key === status.active_server_key && app.connected ? "Подключён" : status.vpn_enabled ? "Ожидание VPN" : "Отключён"}</dd></div></dl>
        {#if server.managed}<a class="text-link" href={serverManagementPath(server.public_key)} onclick={close}>Управление сервером</a>{/if}
        <div class="form-actions"><button class="btn ghost danger" type="button" disabled={busy} onclick={() => flow = { kind: "delete", publicKey: server.public_key }}>Удалить</button><button class="btn primary" type="submit" disabled={busy}>Сохранить</button></div>
      </form>
    {:else if flow.kind === "connect" && server}
      <p class="dialog-intro">Подключить {server.name}? Интернет может ненадолго прерваться.</p><div class="form-actions"><button class="btn ghost" type="button" disabled={busy} onclick={close}>Отмена</button><button class="btn primary" type="button" disabled={busy} onclick={connect}>Подключить</button></div>
    {:else if flow.kind === "disconnect"}
      <p class="dialog-intro">VPN будет отключён. Сайты будут открываться без VPN, правила сохранятся.</p><div class="form-actions"><button class="btn ghost" type="button" disabled={busy} onclick={close}>Отмена</button><button class="btn primary" type="button" disabled={busy} onclick={disconnect}>Отключить</button></div>
    {:else if flow.kind === "delete" && server}
      <p class="dialog-intro">Удалить {server.name}? {server.public_key === status.active_server_key && status.vpn_enabled ? "VPN будет отключён." : ""} {server.managed ? "VPS не будет удалён." : ""}</p><div class="form-actions"><button class="btn ghost" type="button" disabled={busy} onclick={close}>Отмена</button><button class="btn danger" type="button" disabled={busy} onclick={remove}>Удалить</button></div>
    {/if}
    {#if error || app.actionError}<p class="error" role="alert">{error || app.actionError}</p>{/if}
  </Dialog>
{/if}
