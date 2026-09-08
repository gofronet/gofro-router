<script lang="ts">
  import RouterIcon from "lucide-svelte/icons/router";
  import Globe from "lucide-svelte/icons/globe";
  import Power from "lucide-svelte/icons/power";
  import ChevronRight from "lucide-svelte/icons/chevron-right";
  import Laptop from "lucide-svelte/icons/laptop";
  import Smartphone from "lucide-svelte/icons/smartphone";
  import Tablet from "lucide-svelte/icons/tablet";
  import { getAppContext } from "../app-context";
  import { p } from "../router";
  import { formatRate } from "../format";
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
    if (status.vpn_enabled) flow = { kind: "disconnect" };
    else if (server) app.setMode(true);
    else flow = { kind: "choose" };
  }
</script>

<svelte:head><title>Обзор · GofroRouter</title></svelte:head>

<section class="panel connection" aria-labelledby="connection-heading">
  <div class="connection-top">
    <div>
      <h2 id="connection-heading" class="status" class:off={!connected}><span class="dot"></span>{label}</h2>
      <a class="connection-mode" href={p("/routing")}>{!status.vpn_enabled ? "Интернет без VPN" : !connected ? "Ожидаем соединение с VPN" : status.routing.config.mode === "all" ? "Весь интернет через VPN" : "По правилам"}<ChevronRight class="icon" /></a>
    </div>
  </div>
  <button class="connection-power" type="button" aria-label={status.vpn_enabled ? "Отключить VPN" : "Подключить VPN"} aria-pressed={status.vpn_enabled} disabled={app.busy} onclick={toggleVpn}><Power class="icon" /><span>{status.vpn_enabled ? "Вкл" : "Выкл"}</span></button>
  <div class="connection-path" class:connected-path={status.vpn_enabled} aria-label={status.vpn_enabled ? `Ваш роутер, VPN-сервер ${server?.name || ""}, интернет` : "Ваш роутер, интернет"}>
    <div class="path-stop"><span class="path-icon"><RouterIcon class="icon" /></span><strong>Ваш роутер</strong></div>
    {#if status.vpn_enabled}<div class="path-stop"><span class="path-icon"><Globe class="icon" /></span><strong>{server?.name || "VPN-сервер"}</strong></div>{/if}
    <div class="path-stop"><span class="path-icon"><Globe class="icon" /></span><strong>Интернет</strong></div>
  </div>
  <div class="connection-footer">
    <button class="connection-server" type="button" aria-label="Выбрать VPN-сервер" disabled={app.busy} onclick={() => flow = { kind: "choose" }}><ServerMarker emoji={server?.emoji} /><span class="connection-server-name"><strong>{server?.name || "Выбрать сервер"}</strong></span><ChevronRight class="icon" /></button>
  </div>
</section>

<div class="grid-two overview-summary-grid">
  <TrafficPanel />
  <section class="panel connected-devices" aria-labelledby="devices-heading">
    <div class="panel-head"><h2 id="devices-heading">Устройства <span class="count">{status.devices.length}</span></h2><a class="text-link" href={p("/devices")}>Все устройства <ChevronRight class="icon" /></a></div>
    <div class="device-list">
      {#each status.devices.slice(0, 3) as device (device.mac)}
        {@const name = device.hostname || device.ip || "Устройство"}
        <a class="device-row" href={`/?device=${encodeURIComponent(device.mac)}${p("/devices").slice(1)}`}>
          <span class="device-icon">{#if /ipad|tablet/i.test(name)}<Tablet class="icon" />{:else if /iphone|android|phone/i.test(name)}<Smartphone class="icon" />{:else}<Laptop class="icon" />{/if}</span>
          <div class="device-name"><strong>{name}</strong><small class="mono">{device.ip || device.mac}</small></div>
          <span class="device-speed">↓ {formatRate(device.rx_bps)}</span>
        </a>
      {:else}<div class="empty"><p>Нет подключённых Wi-Fi устройств.</p></div>{/each}
    </div>
    <a class="wifi-shortcut" href={p("/wifi")}><h3>Настройки Wi-Fi</h3><ChevronRight class="icon" /></a>
  </section>
</div>
<ServerDialogs bind:flow />
