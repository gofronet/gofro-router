<script lang="ts">
  import { getAppContext } from "../app-context";
  import { formatBytes, formatRate } from "../format";
  import Chart from "./chart.svelte";

  const app = getAppContext();
  const status = $derived(app.status);
  const down = $derived(formatRate(status.stats.rx_bps).split(" "));
  const up = $derived(formatRate(status.stats.tx_bps).split(" "));
</script>

<section class="panel traffic-panel" aria-labelledby="traffic-title">
  <div class="panel-head"><h2 id="traffic-title">Трафик VPN</h2><span class="muted small">Последние 10 минут</span></div>
  <div class="traffic-values">
    <div class="traffic-value"><span>↓ Скачивание</span><strong>{down[0]}<small>{down.slice(1).join(" ")}</small></strong></div>
    <div class="traffic-value"><span>↑ Отдача</span><strong>{up[0]}<small>{up.slice(1).join(" ")}</small></strong></div>
  </div>
  <div class="chart-wrap"><Chart history={status.history} label="Трафик VPN: скачивание и отдача" /></div>
  <div class="chart-note"><span>Данные VPN</span><span>↓ {formatBytes(status.peer?.rx_bytes)} · ↑ {formatBytes(status.peer?.tx_bytes)}</span></div>
</section>
