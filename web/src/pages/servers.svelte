<script lang="ts">
  import MoreHorizontal from "lucide-svelte/icons/more-horizontal";
  import Plus from "lucide-svelte/icons/plus";
  import Power from "lucide-svelte/icons/power";

  import { getAppContext } from "../app-context";
  import ServerDialogs, { type ServerDialogFlow } from "../components/server-dialogs.svelte";
  import { formatAgo } from "../format";
  import { p as path } from "../router";

  const app = getAppContext();
  const status = $derived(app.status);
  const busy = $derived(app.busy);
  const activeServer = $derived(status.servers.find((server) => server.public_key === status.active_server_key));
  let flow = $state<ServerDialogFlow>(null);

  function openConnect(publicKey: string) {
    flow = { kind: "connect", publicKey };
  }
</script>

<svelte:head><title>Серверы · Gofro Router</title></svelte:head>

<section aria-labelledby="servers-title">
  <div class="section-caption">
    <h2 id="servers-title">Ваши серверы <span class="count">{status.servers.length}</span></h2>
    <button class="btn primary" type="button" disabled={busy} onclick={() => flow = { kind: "add" }}><Plus size={17} />Добавить</button>
  </div>

  <section class="panel" aria-label="Сохранённые серверы">
    {#if status.servers.length === 0}
      <div class="empty"><h3>Пока нет серверов</h3><p>Добавьте сервер, чтобы включить VPN.</p></div>
    {:else}
      {#each status.servers as server (server.public_key)}
        {@const selected = server.public_key === status.active_server_key}
        {@const connected = selected && app.connected}
        <article class="full-row">
          <span class="country-code">VPN</span>
          <div class="row-main">
            <h3>{server.name}</h3>
            <p><span class="mono">{server.endpoint}</span> · {server.managed ? "Управляемый VPS" : "Импортированный"}</p>
          </div>
          <div class="row-actions">
            {#if selected && status.vpn_enabled}
              <span class:connected class="tag"><i class="dot"></i>{connected ? "Подключён" : "Подключаемся"}</span>
              <button class="icon-btn" type="button" disabled={busy} aria-label="Отключить VPN" onclick={() => flow = { kind: "disconnect" }}><Power size={19} /></button>
            {:else}
              <button class="btn" type="button" disabled={busy} onclick={() => openConnect(server.public_key)}>{selected && status.vpn_enabled ? "Повторить" : "Подключить"}</button>
            {/if}
            <button class="icon-btn" type="button" disabled={busy} aria-label={`Настройки ${server.name}`} onclick={() => flow = { kind: "edit", publicKey: server.public_key }}><MoreHorizontal size={19} /></button>
          </div>
        </article>
      {/each}
    {/if}
  </section>

  <section class="panel">
    <details class="disclosure">
      <summary>Параметры подключения</summary>
      <div class="details-content">
        <dl class="key-values">
          <div><dt>Сервер</dt><dd>{activeServer?.name || "Не выбран"}</dd></div>
          <div><dt>VPN-интерфейс</dt><dd class="mono">{status.interface}</dd></div>
          <div><dt>Последний ответ сервера</dt><dd>{formatAgo(status.peer?.handshake_age_seconds)}</dd></div>
          <div><dt>Режим VPN</dt><dd><a class="text-link" href={path("/routing")}>Правила маршрутизации</a></dd></div>
        </dl>
      </div>
    </details>
  </section>
</section>

<ServerDialogs bind:flow />
