<script lang="ts">
  import Laptop from "lucide-svelte/icons/laptop";
  import Smartphone from "lucide-svelte/icons/smartphone";
  import Tablet from "lucide-svelte/icons/tablet";
  import ChevronRight from "lucide-svelte/icons/chevron-right";
  import { getAppContext } from "../app-context";
  import { formatBytes, formatDuration, formatRate } from "../format";
  import { p, route } from "../router";

  const app = getAppContext();
  const status = $derived(app.status);
  const selected = $derived(route.search.device);
</script>

<svelte:head><title>Устройства · GofroRouter</title></svelte:head>
<div class="section-caption"><h2>Подключены сейчас <span class="count">{status.devices.length}</span></h2></div>
<section class="panel" aria-label="Подключённые устройства">
  {#each status.devices as device (device.mac)}
    {@const name = device.hostname || device.ip || "Устройство"}
    <details class="disclosure device-disclosure" open={selected === device.mac}>
      <summary><span class="device-icon">{#if /ipad|tablet/i.test(name)}<Tablet class="icon" />{:else if /iphone|android|phone/i.test(name)}<Smartphone class="icon" />{:else}<Laptop class="icon" />{/if}</span><span class="device-name"><strong>{name}</strong><small class="mono">{device.ip || device.mac}</small></span><span class="device-speed">↓ {formatRate(device.rx_bps)}</span></summary>
      <div class="details-content device-detail">
        <dl class="key-values">
          <div><dt>IP-адрес</dt><dd class="mono">{device.ip || "Нет данных"}</dd></div>
          <div><dt>Подключение</dt><dd>Wi-Fi</dd></div>
          <div><dt>VPN</dt><dd>{!status.vpn_enabled ? "Отключён" : !app.connected ? "Нет соединения" : status.routing.config.mode === "all" ? "Весь интернет" : "По общим правилам"}</dd></div>
          <div><dt>MAC-адрес</dt><dd class="mono">{device.mac}</dd></div>
          <div><dt>Подключено</dt><dd>{formatDuration(device.connected_seconds)}</dd></div>
          <div><dt>Сигнал</dt><dd class="mono">{device.signal_dbm == null ? "Нет данных" : `${device.signal_dbm} dBm`}</dd></div>
          <div><dt>Трафик устройства</dt><dd>↓ {formatBytes(device.rx_bytes)} · ↑ {formatBytes(device.tx_bytes)}</dd></div>
        </dl>
        <a class="text-link" href={p("/routing")}>Правила VPN <ChevronRight class="icon" /></a>
      </div>
    </details>
  {:else}<div class="empty"><h3>Нет подключённых устройств</h3><p>Здесь появятся устройства, подключённые к Wi-Fi роутера.</p></div>{/each}
</section>
<p class="support-text">Правила VPN общие для всех устройств.</p>
