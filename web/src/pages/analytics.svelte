<script lang="ts">
  import ChevronRight from "lucide-svelte/icons/chevron-right";
  import { getAppContext } from "../app-context";
  import { formatAgo, formatDuration } from "../format";
  import { p } from "../router";
  import TrafficPanel from "../components/traffic-panel.svelte";
  const app = getAppContext();
  const status = $derived(app.status);
  const percent = (value: number) => value.toLocaleString("ru-RU", { maximumFractionDigits: 1 });
</script>

<svelte:head><title>Диагностика · GofroRouter</title></svelte:head>
<div class="section-caption"><h2>Состояние роутера</h2></div>
<div class="system-grid">
  <section class="panel" aria-labelledby="resources-title">
    <div class="panel-head"><h2 id="resources-title">Нагрузка</h2></div>
    <div class="panel-body">
      <div class="meter-row"><div class="meter-label"><span>Процессор</span><strong>{percent(status.stats.load_percent)} %</strong></div><progress max={100} value={status.stats.load_percent} aria-label="Загрузка процессора"></progress></div>
      <div class="meter-row"><div class="meter-label"><span>Память</span><strong>{percent(status.stats.memory_percent)} %</strong></div><progress max={100} value={status.stats.memory_percent} aria-label="Использование памяти"></progress></div>
      <dl class="key-values"><div><dt>Температура</dt><dd class="mono">{status.stats.temperature_c == null ? "Нет данных" : `${percent(status.stats.temperature_c)} °C`}</dd></div><div><dt>Без перезагрузки</dt><dd>{formatDuration(status.stats.uptime_seconds)}</dd></div></dl>
    </div>
  </section>
  <section class="panel" aria-labelledby="vpn-diagnostics-title">
    <div class="panel-head"><h2 id="vpn-diagnostics-title">Подключение VPN</h2></div>
    <div class="panel-body">
      <dl class="key-values"><div><dt>Состояние</dt><dd>{app.connected ? "Подключён" : status.vpn_enabled ? "Нет соединения" : "Отключён"}</dd></div><div><dt>Последний ответ</dt><dd>{formatAgo(status.peer?.handshake_age_seconds)}</dd></div></dl>
      <details class="disclosure px-0"><summary>Технические данные</summary><div class="details-content"><dl class="key-values"><div><dt>Интерфейс</dt><dd class="mono">{status.interface}</dd></div><div><dt>FakeDNS</dt><dd>{status.routing.dns_active ? "Активен" : "Не активен"}</dd></div><div><dt>FakeIP</dt><dd class="mono">{status.routing.fake_ips}</dd></div><div><dt>Dataplane</dt><dd>{status.routing.dataplane_active ? "Работает" : "Не активен"}</dd></div><div><dt>GeoSite / GeoIP</dt><dd>{status.routing.geosite_loaded ? "Загружен" : "Нет данных"} / {status.routing.geoip_loaded ? "Загружен" : "Нет данных"}</dd></div></dl></div></details>
      <a class="text-link" href={p("/routing")}>Проверить сайт <ChevronRight class="icon" /></a>
    </div>
  </section>
</div>
<div class="mt-5"><TrafficPanel /></div>
