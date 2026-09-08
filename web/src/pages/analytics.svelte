<script lang="ts">
  import { getAppContext } from "../app-context";
  import TrafficPanel from "../components/traffic-panel.svelte";
  const app = getAppContext();
  const status = $derived(app.status);
  const percent = (value: number) => value.toLocaleString("ru-RU", { maximumFractionDigits: 1 });
</script>

<svelte:head><title>Диагностика · GofroRouter</title></svelte:head>
<section class="panel" aria-labelledby="resources-title">
    <div class="panel-head"><h2 id="resources-title">Нагрузка</h2></div>
    <div class="panel-body">
      <div class="meter-row"><div class="meter-label"><span>Процессор</span><strong>{percent(status.stats.load_percent)} %</strong></div><progress max={100} value={status.stats.load_percent} aria-label="Загрузка процессора"></progress></div>
      <div class="meter-row"><div class="meter-label"><span>Память</span><strong>{percent(status.stats.memory_percent)} %</strong></div><progress max={100} value={status.stats.memory_percent} aria-label="Использование памяти"></progress></div>
       <dl class="key-values"><div><dt>Температура</dt><dd class="mono">{status.stats.temperature_c == null ? "Нет данных" : `${percent(status.stats.temperature_c)} °C`}</dd></div></dl>
    </div>
</section>
<div class="mt-5"><TrafficPanel /></div>
