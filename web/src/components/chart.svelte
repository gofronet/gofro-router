<script lang="ts">
  import { onMount, untrack } from "svelte";
  import type {
    Chart as ChartInstance,
    ChartConfiguration,
    ChartDataset,
    ScriptableContext,
  } from "chart.js";
  import { formatRate } from "../format";
  import type { HistoryPoint } from "../domain/models";
  import { appearance } from "../stores/theme.svelte";

  let {
    history,
    label,
  }: {
    history: HistoryPoint[];
    label: string;
  } = $props();

  let canvas: HTMLCanvasElement;
  let chart: ChartInstance<"line"> | null = null;
  let ChartConstructor: typeof import("chart.js").Chart | null = null;
  let loadFailed = $state(false);
  const time = new Intl.DateTimeFormat("ru-RU", {
    hour: "2-digit",
    minute: "2-digit",
  });

  function date(timestamp: number) {
    return new Date(timestamp < 1e12 ? timestamp * 1000 : timestamp);
  }

  function colors() {
    const style = getComputedStyle(document.documentElement);
    return {
      foreground: style.getPropertyValue("--fg").trim(),
      muted: style.getPropertyValue("--muted").trim(),
      border: style.getPropertyValue("--border").trim(),
      surface: style.getPropertyValue("--surface").trim(),
    };
  }

  function fill(color: string) {
    return (context: ScriptableContext<"line">) => {
      const { ctx, chartArea } = context.chart;
      if (!chartArea) return "transparent";
      const gradient = ctx.createLinearGradient(0, chartArea.top, 0, chartArea.bottom);
      gradient.addColorStop(0, `color-mix(in srgb, ${color} 8%, transparent)`);
      gradient.addColorStop(1, "transparent");
      return gradient;
    };
  }

  function datasets(points: HistoryPoint[]): ChartDataset<"line", (number | null)[]>[] {
    const theme = colors();
    const common = {
      borderWidth: 1.8,
      pointRadius: 0,
      pointHoverRadius: 3,
      pointHitRadius: 12,
      tension: 0.3,
    };
    return [
      {
        ...common,
        label: "↓ Скачивание (RX)",
        data: points.map((point) => point.rx_bps),
        borderColor: theme.foreground,
        backgroundColor: fill(theme.foreground),
        fill: true,
      },
      {
        ...common,
        label: "↑ Отдача (TX)",
        data: points.map((point) => point.tx_bps),
        borderColor: theme.muted,
        backgroundColor: fill(theme.muted),
        fill: true,
      },
    ];
  }

  function config(points: HistoryPoint[]): ChartConfiguration<"line"> {
    const theme = colors();
    return {
      type: "line",
      data: {
        labels: points.map((point) => time.format(date(point.timestamp))),
        datasets: datasets(points),
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        animation: false,
        normalized: true,
        interaction: { mode: "index", intersect: false },
        layout: { padding: { top: 2, right: 2 } },
        plugins: {
          legend: { display: false },
          tooltip: {
            backgroundColor: theme.foreground,
            borderColor: theme.border,
            borderWidth: 1,
            titleColor: theme.surface,
            bodyColor: theme.surface,
            padding: 9,
            callbacks: {
              label: (item) => {
                const value = item.parsed.y;
                return `${item.dataset.label}: ${value === null ? "нет данных" : formatRate(value)}`;
              },
            },
          },
        },
        scales: {
          x: {
            grid: { display: false },
            border: { color: theme.border },
            ticks: {
              color: theme.muted,
              maxTicksLimit: 3,
              maxRotation: 0,
              font: { family: "SFMono-Regular, Consolas, monospace", size: 10 },
            },
          },
          y: {
            beginAtZero: true,
            grid: { color: theme.border },
            border: { display: false },
            ticks: {
              color: theme.muted,
              maxTicksLimit: 3,
              callback: (value) => formatRate(Number(value)).replace("/с", ""),
              font: { family: "SFMono-Regular, Consolas, monospace", size: 10 },
            },
          },
        },
      },
    };
  }

  function createChart() {
    if (!ChartConstructor) return;
    chart?.destroy();
    chart = new ChartConstructor(canvas, config(history));
  }

  onMount(() => {
    let mounted = true;
    const media = matchMedia("(prefers-color-scheme: dark)");
    const onMediaChange = () => {
      if (appearance.theme === "system") createChart();
    };
    media.addEventListener("change", onMediaChange);

    void import("chart.js")
      .then((module) => {
        if (!mounted) return;
        const {
          CategoryScale,
          Chart,
          Filler,
          LinearScale,
          LineController,
          LineElement,
          PointElement,
          Tooltip,
        } = module;
        Chart.register(
          CategoryScale,
          LinearScale,
          LineController,
          LineElement,
          PointElement,
          Filler,
          Tooltip,
        );
        ChartConstructor = Chart;
        createChart();
      })
      .catch(() => {
        if (mounted) loadFailed = true;
      });

    return () => {
      mounted = false;
      media.removeEventListener("change", onMediaChange);
      chart?.destroy();
      chart = null;
    };
  });

  $effect(() => {
    appearance.theme;
    untrack(createChart);
  });

  $effect(() => {
    const points = history;
    if (!chart) return;
    chart.data.labels = points.map((point) => time.format(date(point.timestamp)));
    chart.data.datasets = datasets(points);
    chart.update("none");
  });
</script>

<div class="relative h-[152px] w-full">
  <canvas bind:this={canvas} aria-label={`${label}. RX: скачивание; TX: отдача.`}>{label}</canvas>
  {#if loadFailed || history.length === 0}
    <div class="absolute inset-x-1 bottom-6 top-3 grid place-items-center rounded-lg border border-dashed border-[var(--border)] px-4 text-center text-xs text-[var(--muted)]">
      {loadFailed ? "Обновите страницу для загрузки графика" : "График появится после первых замеров"}
    </div>
  {/if}
</div>
