<script lang="ts">
  import { onMount } from "svelte";
  import {
    CategoryScale,
    Chart,
    Filler,
    Legend,
    LinearScale,
    LineController,
    LineElement,
    PointElement,
    Tooltip,
    type ChartConfiguration,
    type ChartDataset,
    type ScriptableContext,
  } from "chart.js";
  import { formatRate } from "../format";
  import type { HistoryPoint } from "../domain/models";

  let {
    history,
    kind,
    label,
  }: {
    history: HistoryPoint[];
    kind: "traffic" | "system";
    label: string;
  } = $props();

  Chart.register(
    CategoryScale,
    LinearScale,
    LineController,
    LineElement,
    PointElement,
    Filler,
    Tooltip,
    Legend,
  );

  let canvas: HTMLCanvasElement;
  let chart: Chart<"line"> | null = null;
  const time = new Intl.DateTimeFormat("ru-RU", {
    hour: "2-digit",
    minute: "2-digit",
  });

  function date(timestamp: number) {
    return new Date(timestamp < 1e12 ? timestamp * 1000 : timestamp);
  }

  function fill(top: string, bottom: string) {
    return (context: ScriptableContext<"line">) => {
      const { ctx, chartArea } = context.chart;
      if (!chartArea) return bottom;
      const gradient = ctx.createLinearGradient(
        0,
        chartArea.top,
        0,
        chartArea.bottom,
      );
      gradient.addColorStop(0, top);
      gradient.addColorStop(1, bottom);
      return gradient;
    };
  }

  function datasets(
    points: HistoryPoint[],
  ): ChartDataset<"line", (number | null)[]>[] {
    const common = {
      borderWidth: 2,
      pointRadius: 0,
      pointHoverRadius: 4,
      pointHitRadius: 14,
      tension: 0.34,
    };
    if (kind === "traffic") {
      return [
        {
          ...common,
          label: "Скачивание",
          data: points.map((point) => point.rx_bps),
          borderColor: "#09090b",
          backgroundColor: fill("rgba(9, 9, 11, .13)", "rgba(9, 9, 11, 0)"),
          fill: true,
        },
        {
          ...common,
          label: "Отдача",
          data: points.map((point) => point.tx_bps),
          borderColor: "#8a8a92",
          backgroundColor: fill(
            "rgba(138, 138, 146, .08)",
            "rgba(138, 138, 146, 0)",
          ),
          fill: true,
        },
      ];
    }
    return [
      {
        ...common,
        label: "CPU",
        data: points.map((point) => point.load_percent),
        borderColor: "#09090b",
      },
      {
        ...common,
        label: "RAM",
        data: points.map((point) => point.memory_percent),
        borderColor: "#73737b",
      },
      {
        ...common,
        label: "Температура",
        data: points.map((point) => point.temperature_c),
        borderColor: "#b6b6bc",
        spanGaps: true,
      },
    ];
  }

  function config(points: HistoryPoint[]): ChartConfiguration<"line"> {
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
        layout: { padding: { top: 6 } },
        plugins: {
          legend: {
            align: "start",
            labels: {
              color: "#74747d",
              boxWidth: 18,
              boxHeight: 2,
              padding: 18,
              font: { size: 11 },
            },
          },
          tooltip: {
            backgroundColor: "rgba(9, 9, 11, .96)",
            borderColor: "#343438",
            borderWidth: 1,
            titleColor: "#f4f2e9",
            bodyColor: "#d7d7da",
            padding: 12,
            callbacks: {
              label: (item) => {
                const value = item.parsed.y;
                if (value === null) return `${item.dataset.label}: нет данных`;
                if (kind === "traffic")
                  return `${item.dataset.label}: ${formatRate(value)}`;
                return `${item.dataset.label}: ${value.toLocaleString("ru-RU", { maximumFractionDigits: 1 })}${item.dataset.label === "Температура" ? " °C" : " %"}`;
              },
            },
          },
        },
        scales: {
          x: {
            grid: { display: false },
            border: { color: "#dedee1" },
            ticks: {
              color: "#8a8a92",
              maxTicksLimit: 5,
              maxRotation: 0,
              font: { size: 10 },
            },
          },
          y: {
            beginAtZero: true,
            suggestedMax: kind === "system" ? 100 : undefined,
            grid: { color: "rgba(9, 9, 11, .08)" },
            border: { display: false },
            ticks: {
              color: "#8a8a92",
              maxTicksLimit: 5,
              callback: (value) =>
                kind === "traffic"
                  ? formatRate(Number(value)).replace("/с", "")
                  : `${value}%`,
              font: { size: 10 },
            },
          },
        },
      },
    };
  }

  onMount(() => {
    chart = new Chart(canvas, config(history));
    return () => chart?.destroy();
  });

  $effect(() => {
    if (!chart) return;
    chart.data.labels = history.map((point) =>
      time.format(date(point.timestamp)),
    );
    chart.data.datasets = datasets(history);
    chart.update("none");
  });
</script>

<div
  class={`relative w-full ${kind === "system" ? "h-57.5 lg:h-65" : "h-65 lg:h-82.5"}`}
>
  <canvas bind:this={canvas} aria-label={label}>{label}</canvas>
  {#if history.length === 0}
    <div
      class="absolute inset-x-1 bottom-6 top-14 grid place-items-center rounded-2xl border border-dashed border-[#dedee1] text-center text-xs text-[#74747d]"
    >
      График появится после первых замеров
    </div>
  {/if}
</div>
