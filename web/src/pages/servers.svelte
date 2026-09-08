<script lang="ts">
  import MoreHorizontal from "lucide-svelte/icons/more-horizontal";
  import ChevronRight from "lucide-svelte/icons/chevron-right";
  import Plus from "lucide-svelte/icons/plus";
  import Power from "lucide-svelte/icons/power";

  import { getAppContext } from "../app-context";
  import ServerDialogs, { type ServerDialogFlow } from "../components/server-dialogs.svelte";
  import { serverManagementPath } from "../router";
  import ServerMarker from "../components/server-marker.svelte";

  const app = getAppContext();
  const status = $derived(app.status);
  const busy = $derived(app.busy);
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
      <div class="empty"><h3>Пока нет серверов</h3></div>
    {:else}
      {#each status.servers as server (server.public_key)}
        {@const selected = server.public_key === status.active_server_key}
        {@const connected = selected && app.connected}
        <article class="full-row">
          <ServerMarker emoji={server.emoji} />
          <div class="row-main">
            <h3><a class="server-detail-link" href={serverManagementPath(server.public_key)}>{server.name}<ChevronRight class="icon" /></a></h3>
            <p>{#if server.managed}Свой VPS · {/if}<span class="mono">{server.endpoint}</span></p>
          </div>
          <div class="row-actions">
            {#if selected && status.vpn_enabled}
              <span class:connected class:disconnected={!connected} class="tag server-status"><span class="dot" aria-hidden="true"></span>{connected ? "Подключён" : "Подключаемся"}</span>
              <button class="icon-btn" type="button" disabled={busy} aria-label="Отключить VPN" onclick={() => flow = { kind: "disconnect" }}><Power size={19} /></button>
            {:else}
              <span class="tag server-status disconnected"><i class="dot"></i>Отключён</span><button class="icon-btn" type="button" disabled={busy} aria-label={`Подключить ${server.name}`} onclick={() => openConnect(server.public_key)}><Power size={19} /></button>
            {/if}
            <button class="icon-btn" type="button" disabled={busy} aria-label={`Настройки ${server.name}`} onclick={() => flow = { kind: "edit", publicKey: server.public_key }}><MoreHorizontal size={19} /></button>
          </div>
        </article>
      {/each}
    {/if}
  </section>

</section>

<ServerDialogs bind:flow />
