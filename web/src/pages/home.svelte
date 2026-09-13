<script lang="ts">
  import RouterIcon from "lucide-svelte/icons/router";
  import Globe from "lucide-svelte/icons/globe";
  import Power from "lucide-svelte/icons/power";
  import ChevronRight from "lucide-svelte/icons/chevron-right";
  import { getAppContext } from "../app-context";
  import { p } from "../router";
  import ServerDialogs, { type ServerDialogFlow } from "../components/server-dialogs.svelte";
  import ServerMarker from "../components/server-marker.svelte";
  import TrafficPanel from "../components/traffic-panel.svelte";

  const app = getAppContext();
  const status = $derived(app.status);
  const server = $derived(status.servers.find(item => item.public_key === status.active_server_key));
  const connected = $derived(app.connected);
  const label = $derived(connected ? "VPN подключён" : status.vpn_enabled ? "Нет соединения с VPN" : "VPN отключён");
  let flow = $state<ServerDialogFlow>(null);

  function toggleVpn() {
    if (!app.requireFreshStatus()) return;
    if (status.vpn_enabled) flow = { kind: "disconnect" };
    else if (server) app.setMode(true);
    else flow = { kind: "choose" };
  }
</script>

<svelte:head><title>Обзор · Gofro VPN</title></svelte:head>

<section class="panel connection" aria-labelledby="connection-heading">
  <div class="connection-top">
    <div>
      <h2 id="connection-heading" class="status" class:off={!connected}><span class="dot"></span>{label}</h2>
      <a class="connection-mode" href={p("/routing")}>{!status.vpn_enabled ? "Интернет без VPN" : !connected ? "Ожидаем соединение с VPN" : status.routing.config.mode === "all" ? "Весь интернет через VPN" : "По правилам"}<ChevronRight class="icon" /></a>
    </div>
  </div>
  <button class="connection-power" type="button" aria-label={status.vpn_enabled ? "Отключить VPN" : "Подключить VPN"} aria-pressed={status.vpn_enabled} disabled={app.busy} onclick={toggleVpn}><Power class="icon" /><span>{status.vpn_enabled ? "Вкл" : "Выкл"}</span></button>
  <div class="connection-path" class:connected-path={status.vpn_enabled} aria-label={status.vpn_enabled ? `Gofro VPN, сервер ${server?.name || ""}, интернет` : "Gofro VPN, интернет"}>
    <div class="path-stop"><span class="path-icon"><RouterIcon class="icon" /></span><strong>Gofro VPN</strong></div>
    {#if status.vpn_enabled}<div class="path-stop"><span class="path-icon"><Globe class="icon" /></span><strong>{server?.name || "VPN-сервер"}</strong></div>{/if}
    <div class="path-stop"><span class="path-icon"><Globe class="icon" /></span><strong>Интернет</strong></div>
  </div>
  <div class="connection-footer">
    <button class="connection-server" type="button" aria-label="Выбрать VPN-сервер" disabled={app.busy} onclick={() => flow = { kind: "choose" }}><ServerMarker emoji={server?.emoji} /><span class="connection-server-name"><strong>{server?.name || "Выбрать сервер"}</strong></span><ChevronRight class="icon" /></button>
  </div>
</section>

<div class="overview-summary-grid"><TrafficPanel /></div>
<ServerDialogs bind:flow />
