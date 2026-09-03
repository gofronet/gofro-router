<script lang="ts">
  import ArrowDown from "lucide-svelte/icons/arrow-down";
  import ArrowUp from "lucide-svelte/icons/arrow-up";
  import ChevronDown from "lucide-svelte/icons/chevron-down";
  import Clock3 from "lucide-svelte/icons/clock-3";
  import Cpu from "lucide-svelte/icons/cpu";
  import HardDriveDownload from "lucide-svelte/icons/hard-drive-download";
  import MemoryStick from "lucide-svelte/icons/memory-stick";
  import Thermometer from "lucide-svelte/icons/thermometer";
  import Wifi from "lucide-svelte/icons/wifi";

  import { Chart } from "../components";
  import { getAppContext } from "../app-context";
  import {
    formatAgo,
    formatBytes,
    formatDuration,
    formatRate,
  } from "../format";
  import type { Device } from "../domain/models";

  const app = getAppContext();
  const status = $derived(app.status);
  let expanded = $state<string | null>(null);

  function deviceName(device: Device) {
    return (
      device.hostname?.trim() ||
      (device.ip
        ? `Устройство ${device.ip}`
        : `Устройство ${device.mac.slice(-5)}`)
    );
  }

  function signalQuality(dbm: number | null) {
    if (dbm == null) return null;
    return Math.max(0, Math.min(100, Math.round((dbm + 100) * 2)));
  }

  function inactivity(ms: number) {
    if (ms < 5000) return "сейчас";
    return formatAgo(ms / 1000);
  }
</script>

<svelte:head><title>Аналитика · Gofro Router</title></svelte:head>

<section class="grid min-w-0 gap-5 lg:gap-6" aria-labelledby="analytics-title">
  <header
    class="min-w-0 px-0.5 py-2 sm:flex sm:items-end sm:justify-between sm:gap-6"
  >
    <div class="min-w-0">
      <span class="text-xs font-bold tracking-[0.18em] text-[#74747d] uppercase"
        >Обновление каждые 5 секунд</span
      >
      <h1
        class="mt-2 text-[clamp(2.25rem,11vw,3.25rem)] leading-[0.98] font-extrabold tracking-[-0.06em] lg:text-[clamp(3rem,5vw,4.2rem)]"
        id="analytics-title"
      >
        Аналитика сети
      </h1>
      <p class="mt-3.5 max-w-2xl text-base leading-relaxed text-[#74747d]">
        Скорость, ресурсы роутера и подключенные устройства.
      </p>
    </div>
    <span
      class="mt-3 block shrink-0 text-xs font-semibold text-[#74747d] sm:mb-1.5 sm:mt-0"
      >Все Wi-Fi сети</span
    >
  </header>

  <article
    class="min-w-0 overflow-hidden rounded-[28px] border border-[#dedee1] bg-white px-4 pb-3 pt-5 shadow-sm sm:px-6 sm:pt-6"
  >
    <div class="mb-4 flex min-w-0 items-start justify-between gap-3 px-1">
      <div class="min-w-0">
        <span
          class="text-xs font-bold tracking-[0.18em] text-[#74747d] uppercase"
          >Общий трафик</span
        >
        <h2 class="mt-1.5 text-xl font-bold tracking-[-0.035em]">
          Пропускная способность
        </h2>
      </div>
      <div class="flex shrink-0 items-center gap-2 text-xs font-bold">
        <i class="size-2 bg-current"></i>Live
      </div>
    </div>
    <Chart
      history={status.history}
      kind="traffic"
      label="График скачивания и отдачи"
    />
  </article>

  <div class="grid min-w-0 gap-3 sm:grid-cols-3">
    <article
      class="grid min-h-28 min-w-0 grid-cols-[3.125rem_minmax(0,1fr)] items-center gap-x-3.5 rounded-[28px] border border-[#dedee1] bg-white p-4 shadow-sm sm:grid-cols-1"
    >
      <span
        class="row-span-2 grid size-12.5 place-items-center rounded-2xl bg-[#f0f0f2] sm:row-auto sm:mb-2"
        ><ArrowDown size={19} /></span
      >
      <span class="text-xs text-[#74747d]">Получено через VPN</span>
      <strong class="text-lg font-bold"
        >{formatBytes(status.peer?.rx_bytes)}</strong
      >
    </article>
    <article
      class="grid min-h-28 min-w-0 grid-cols-[3.125rem_minmax(0,1fr)] items-center gap-x-3.5 rounded-[28px] border border-[#dedee1] bg-white p-4 shadow-sm sm:grid-cols-1"
    >
      <span
        class="row-span-2 grid size-12.5 place-items-center rounded-2xl bg-[#f0f0f2] sm:row-auto sm:mb-2"
        ><ArrowUp size={19} /></span
      >
      <span class="text-xs text-[#74747d]">Отправлено через VPN</span>
      <strong class="text-lg font-bold"
        >{formatBytes(status.peer?.tx_bytes)}</strong
      >
    </article>
    <article
      class="grid min-h-28 min-w-0 grid-cols-[3.125rem_minmax(0,1fr)] items-center gap-x-3.5 rounded-[28px] border border-[#dedee1] bg-white p-4 shadow-sm sm:grid-cols-1"
    >
      <span
        class="row-span-2 grid size-12.5 place-items-center rounded-2xl bg-[#f0f0f2] sm:row-auto sm:mb-2"
        ><Clock3 size={19} /></span
      >
      <span class="text-xs text-[#74747d]">Время работы</span>
      <strong class="text-lg font-bold"
        >{formatDuration(status.stats.uptime_seconds)}</strong
      >
    </article>
  </div>

  <div
    class="grid min-w-0 gap-3 lg:grid-cols-[minmax(0,1.45fr)_minmax(330px,0.55fr)]"
  >
    <article
      class="min-w-0 overflow-hidden rounded-[28px] border border-[#dedee1] bg-white px-4 pb-3 pt-5 shadow-sm sm:px-6 sm:pt-6"
    >
      <div class="mb-4 px-1">
        <span
          class="text-xs font-bold tracking-[0.18em] text-[#74747d] uppercase"
          >Состояние узла</span
        >
        <h2 class="mt-1.5 text-xl font-bold tracking-[-0.035em]">
          Ресурсы системы
        </h2>
      </div>
      <Chart
        history={status.history}
        kind="system"
        label="График процессора, памяти и температуры"
      />
    </article>

    <div class="grid min-w-0 grid-cols-2 gap-3 sm:grid-cols-4 lg:grid-cols-2">
      <article
        class="grid min-h-32 min-w-0 grid-cols-[1.375rem_minmax(0,1fr)] gap-2 rounded-[28px] border border-[#dedee1] bg-white p-4 shadow-sm lg:min-h-36"
      >
        <Cpu size={20} /><span class="text-xs text-[#74747d]">Процессор</span
        ><strong
          class="col-span-2 overflow-hidden text-ellipsis whitespace-nowrap text-base font-bold"
          >{status.stats.load_percent.toLocaleString("ru-RU", {
            maximumFractionDigits: 1,
          })}%</strong
        >
        <div class="col-span-2 h-1 self-end overflow-hidden bg-[#f0f0f2]">
          <i
            class="block h-full bg-[#09090b]"
            style:width={`${Math.min(status.stats.load_percent, 100)}%`}
          ></i>
        </div>
      </article>
      <article
        class="grid min-h-32 min-w-0 grid-cols-[1.375rem_minmax(0,1fr)] gap-2 rounded-[28px] border border-[#dedee1] bg-white p-4 shadow-sm lg:min-h-36"
      >
        <MemoryStick size={20} /><span class="text-xs text-[#74747d]"
          >Память</span
        ><strong
          class="col-span-2 overflow-hidden text-ellipsis whitespace-nowrap text-base font-bold"
          >{status.stats.memory_percent.toLocaleString("ru-RU", {
            maximumFractionDigits: 1,
          })}%</strong
        >
        <div class="col-span-2 h-1 self-end overflow-hidden bg-[#f0f0f2]">
          <i
            class="block h-full bg-[#09090b]"
            style:width={`${Math.min(status.stats.memory_percent, 100)}%`}
          ></i>
        </div>
      </article>
      <article
        class="grid min-h-32 min-w-0 grid-cols-[1.375rem_minmax(0,1fr)] gap-2 rounded-[28px] border border-[#dedee1] bg-white p-4 shadow-sm lg:min-h-36"
      >
        <Thermometer size={20} /><span class="text-xs text-[#74747d]"
          >Температура</span
        ><strong
          class="col-span-2 overflow-hidden text-ellipsis whitespace-nowrap text-base font-bold"
          >{status.stats.temperature_c == null
            ? "Нет датчика"
            : `${status.stats.temperature_c.toLocaleString("ru-RU", { maximumFractionDigits: 1 })} °C`}</strong
        >
      </article>
      <article
        class="grid min-h-32 min-w-0 grid-cols-[1.375rem_minmax(0,1fr)] gap-2 rounded-[28px] border border-[#dedee1] bg-white p-4 shadow-sm lg:min-h-36"
      >
        <HardDriveDownload size={20} /><span class="text-xs text-[#74747d]"
          >VPN-интерфейс</span
        ><strong
          class="col-span-2 overflow-hidden text-ellipsis whitespace-nowrap text-base font-bold"
          >{status.interface || "Нет данных"}</strong
        >
      </article>
    </div>
  </div>

  <section class="mt-2 grid min-w-0 gap-3" aria-labelledby="devices-title">
    <header class="flex min-w-0 items-end justify-between gap-3 px-1">
      <div class="min-w-0">
        <span
          class="text-xs font-bold tracking-[0.18em] text-[#74747d] uppercase"
          >Wi-Fi клиенты</span
        >
        <h2
          class="mt-1.5 text-xl font-bold tracking-[-0.035em]"
          id="devices-title"
        >
          Подключенные устройства
        </h2>
      </div>
      <span class="shrink-0 text-xs font-semibold text-[#74747d]"
        >{status.stats.wifi_clients} онлайн</span
      >
    </header>

    {#if status.devices.length === 0}
      <div
        class="flex min-h-52 flex-col items-center justify-center gap-2 rounded-[28px] border border-[#dedee1] bg-white p-8 text-center text-[#74747d] shadow-sm"
      >
        <Wifi size={28} /><strong class="mt-1 text-[#09090b]"
          >Устройств пока нет</strong
        ><span class="max-w-sm text-sm leading-relaxed"
          >Подключенные клиенты появятся здесь автоматически.</span
        >
      </div>
    {:else}
      <div class="grid min-w-0 items-start gap-3 sm:grid-cols-2 xl:grid-cols-3">
        {#each status.devices as device (device.mac)}
          {@const open = expanded === device.mac}
          {@const quality = signalQuality(device.signal_dbm)}
          <article
            class={`min-w-0 overflow-hidden rounded-[28px] border bg-white shadow-sm ${open ? "border-[#09090b]" : "border-[#dedee1]"}`}
          >
            <button
              class="grid min-h-22 w-full min-w-0 grid-cols-[3rem_minmax(0,1fr)_1.25rem] items-center gap-3 border-0 bg-transparent px-4 py-3 text-left min-[440px]:grid-cols-[3rem_minmax(0,1fr)_auto_1.25rem]"
              type="button"
              aria-expanded={open}
              aria-controls={`device-${device.mac.replaceAll(":", "")}`}
              onclick={() => (expanded = open ? null : device.mac)}
            >
              <span
                class="relative grid size-12 place-items-center overflow-hidden rounded-2xl bg-[#f0f0f2]"
                ><Wifi size={21} /><i
                  class="absolute inset-x-0 bottom-0 h-0.75 bg-[#09090b]"
                  style:width={quality == null ? "0" : `${quality}%`}
                ></i></span
              >
              <span class="min-w-0">
                <strong
                  class="block overflow-hidden text-ellipsis whitespace-nowrap text-sm"
                  >{deviceName(device)}</strong
                >
                <small
                  class="mt-1.5 block overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[0.61rem] text-[#74747d]"
                  >{device.ip || "IP не назначен"} · {device.mac}</small
                >
              </span>
              <span
                class="hidden shrink-0 font-mono text-[0.68rem] font-semibold min-[440px]:block"
                ><small class="mr-1">↓</small>{formatRate(device.tx_bps)}</span
              >
              <ChevronDown
                class={open ? "rotate-180 text-[#74747d]" : "text-[#74747d]"}
                size={20}
              />
            </button>

            {#if open}
              <div
                class="border-t border-[#ececef] px-4 pb-4"
                id={`device-${device.mac.replaceAll(":", "")}`}
              >
                <div class="py-3 text-[0.65rem] leading-relaxed text-[#74747d]">
                  Скачивание: точка → устройство (TX) · Отдача: устройство →
                  точка (RX)
                </div>
                <div class="grid gap-2 sm:grid-cols-2">
                  <div
                    class="grid min-w-0 grid-cols-[2.5rem_1fr] items-center gap-x-2.5 rounded-2xl bg-[#f0f0f2] p-3.5"
                  >
                    <span
                      class="row-span-3 grid size-10 place-items-center rounded-xl bg-white"
                      ><ArrowDown size={16} /></span
                    ><small class="text-[0.63rem] text-[#74747d]"
                      >Скачивание сейчас</small
                    ><strong
                      class="overflow-hidden text-ellipsis whitespace-nowrap text-xs"
                      >{formatRate(device.tx_bps)}</strong
                    ><span class="text-[0.63rem] text-[#74747d]"
                      >Всего {formatBytes(device.tx_bytes)}</span
                    >
                  </div>
                  <div
                    class="grid min-w-0 grid-cols-[2.5rem_1fr] items-center gap-x-2.5 rounded-2xl bg-[#f0f0f2] p-3.5"
                  >
                    <span
                      class="row-span-3 grid size-10 place-items-center rounded-xl bg-white"
                      ><ArrowUp size={16} /></span
                    ><small class="text-[0.63rem] text-[#74747d]"
                      >Отдача сейчас</small
                    ><strong
                      class="overflow-hidden text-ellipsis whitespace-nowrap text-xs"
                      >{formatRate(device.rx_bps)}</strong
                    ><span class="text-[0.63rem] text-[#74747d]"
                      >Всего {formatBytes(device.rx_bytes)}</span
                    >
                  </div>
                </div>
                <dl class="mt-3 grid grid-cols-2">
                  <div class="min-w-0 border-t border-[#ececef] px-1.5 py-3">
                    <dt class="text-[0.65rem] text-[#74747d]">Сигнал</dt>
                    <dd class="mt-1.5 wrap-break-word font-mono text-[0.68rem]">
                      {device.signal_dbm == null
                        ? "Нет данных"
                        : `${quality}% · ${device.signal_dbm} dBm`}
                    </dd>
                  </div>
                  <div class="min-w-0 border-t border-[#ececef] px-1.5 py-3">
                    <dt class="text-[0.65rem] text-[#74747d]">Линия RX / TX</dt>
                    <dd class="mt-1.5 wrap-break-word font-mono text-[0.68rem]">
                      {device.rx_bitrate_mbps == null
                        ? "—"
                        : `${device.rx_bitrate_mbps} Мбит/с`} / {device.tx_bitrate_mbps ==
                      null
                        ? "—"
                        : `${device.tx_bitrate_mbps} Мбит/с`}
                    </dd>
                  </div>
                  <div class="min-w-0 border-t border-[#ececef] px-1.5 py-3">
                    <dt class="text-[0.65rem] text-[#74747d]">Подключено</dt>
                    <dd class="mt-1.5 font-mono text-[0.68rem]">
                      {formatDuration(device.connected_seconds)}
                    </dd>
                  </div>
                  <div class="min-w-0 border-t border-[#ececef] px-1.5 py-3">
                    <dt class="text-[0.65rem] text-[#74747d]">Активность</dt>
                    <dd class="mt-1.5 font-mono text-[0.68rem]">
                      {inactivity(device.inactive_ms)}
                    </dd>
                  </div>
                </dl>
              </div>
            {/if}
          </article>
        {/each}
      </div>
    {/if}
  </section>
</section>
