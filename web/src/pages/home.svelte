<script lang="ts">
  import ArrowDown from "lucide-svelte/icons/arrow-down";
  import ArrowUp from "lucide-svelte/icons/arrow-up";
  import Cpu from "lucide-svelte/icons/cpu";
  import MemoryStick from "lucide-svelte/icons/memory-stick";
  import Radio from "lucide-svelte/icons/radio";
  import ServerIcon from "lucide-svelte/icons/server";
  import Thermometer from "lucide-svelte/icons/thermometer";
  import Wifi from "lucide-svelte/icons/wifi";
  import MetricsChart from "../components/chart.svelte";

  import { getAppContext } from "../app-context";
  import { formatAgo, formatRate } from "../format";
  import { p } from "../router";

  const app = getAppContext();
  const status = $derived(app.status);
  const busy = $derived(app.busy);
  const mutation = $derived(app.mutation);

  const activeServer = $derived(
    status.servers.find(
      (server) => server.public_key === status.active_server_key,
    ),
  );

  async function selectServer(event: Event) {
    const publicKey = (event.currentTarget as HTMLSelectElement).value;
    if (!publicKey || publicKey === status.active_server_key) return;
    await app.selectServer(publicKey);
  }
</script>

<svelte:head><title>Главная · Gofro Router</title></svelte:head>

<section
  class="mx-auto grid max-w-6xl min-w-0 gap-5 lg:gap-6"
  aria-labelledby="home-title"
>
  <header
    class="min-w-0 px-0.5 py-2 sm:flex sm:items-end sm:justify-between sm:gap-6"
  >
    <div class="min-w-0">
      <span class="text-xs font-bold tracking-[0.18em] text-[#74747d] uppercase"
        >Сводка</span
      >
      <h1
        class="mt-2 text-[clamp(2.25rem,11vw,3.25rem)] leading-[0.98] font-extrabold tracking-[-0.06em] lg:text-[clamp(3rem,5vw,4.2rem)]"
        id="home-title"
      >
        Обзор сети
      </h1>
      <p class="mt-3.5 max-w-2xl text-base leading-relaxed text-[#74747d]">
        Маршрут, трафик и состояние Gofro Router в одном месте.
      </p>
    </div>
    <span
      class={`mt-4 flex shrink-0 items-center gap-2 text-xs font-bold sm:mb-1.5 sm:mt-0 ${status.tunnel_active ? "text-[#09090b]" : "text-[#74747d]"}`}
    >
      <i class="size-2 bg-current"></i>{status.tunnel_active
        ? "VPN работает"
        : status.vpn_enabled
          ? "VPN недоступен"
          : "DIRECT"}
    </span>
  </header>

  <div
    class="grid min-w-0 gap-3.5 lg:grid-cols-[minmax(320px,0.85fr)_minmax(380px,1.15fr)]"
  >
    <article
      class="min-w-0 overflow-hidden rounded-[28px] bg-[linear-gradient(145deg,#202024,#09090b_72%)] p-6 text-white shadow-xl shadow-black/10"
    >
      <div class="mb-7 flex items-center gap-3.5">
        <span
          class="grid size-12 shrink-0 place-items-center rounded-2xl bg-white text-[#09090b]"
          ><ServerIcon size={21} /></span
        >
        <div class="min-w-0">
          <span class="text-xs text-[#aaaab1]">Маршрут</span>
          <h2 class="mt-1 text-xl font-bold tracking-[-0.035em]">
            Активный сервер
          </h2>
        </div>
      </div>

      {#if status.servers.length > 0}
        <label class="mb-2 block text-xs text-[#aaaab1]" for="home-server"
          >VPN-профиль</label
        >
        <select
          class="h-15 w-full min-w-0 rounded-2xl border-0 bg-white px-4 text-base font-bold text-[#09090b]"
          id="home-server"
          value={status.active_server_key || ""}
          disabled={busy}
          onchange={selectServer}
        >
          {#each status.servers as server (server.public_key)}
            <option value={server.public_key}>{server.name}</option>
          {/each}
        </select>
        <div
          class="mt-4 grid min-w-0 gap-2 font-mono text-[0.66rem] leading-relaxed text-[#98989f]"
        >
          <span
            class="block min-w-0 overflow-hidden text-ellipsis whitespace-nowrap"
            >{activeServer?.endpoint || "Endpoint не задан"}</span
          >
          <span
            class="flex min-w-0 items-center gap-2 overflow-hidden text-ellipsis whitespace-nowrap"
            ><Radio size={15} />Handshake {formatAgo(
              status.peer?.handshake_age_seconds,
            )}</span
          >
        </div>
        {#if mutation?.startsWith("select:")}<small
            class="mt-3 block text-xs text-[#aaaab1]"
            >Переключаем маршрут…</small
          >{/if}
      {:else}
        <p class="mt-3 text-xs text-[#aaaab1]">Нет сохранённых VPN-профилей.</p>
        <a
          class="mt-3 inline-block text-xs font-bold text-white underline underline-offset-4"
          href={p("/servers")}>Добавить сервер</a
        >
      {/if}
    </article>

    <div class="grid min-w-0 grid-cols-2 gap-3">
      <article
        class="grid min-h-28 min-w-0 grid-cols-[2.75rem_minmax(0,1fr)] items-center gap-x-3 rounded-[28px] border border-[#dedee1] bg-white p-4 shadow-sm"
      >
        <span
          class="grid size-11 place-items-center rounded-[14px] bg-[#f0f0f2]"
          ><ArrowDown size={19} /></span
        >
        <div class="flex min-w-0 flex-col justify-center gap-1">
          <span class="text-[0.68rem] text-[#74747d]">Скачивание</span><strong
            class="overflow-hidden text-ellipsis whitespace-nowrap text-[clamp(0.78rem,3.5vw,1.02rem)] font-bold"
            >{formatRate(status.stats.rx_bps)}</strong
          >
        </div>
      </article>
      <article
        class="grid min-h-28 min-w-0 grid-cols-[2.75rem_minmax(0,1fr)] items-center gap-x-3 rounded-[28px] border border-[#dedee1] bg-white p-4 shadow-sm"
      >
        <span
          class="grid size-11 place-items-center rounded-[14px] bg-[#f0f0f2]"
          ><ArrowUp size={19} /></span
        >
        <div class="flex min-w-0 flex-col justify-center gap-1">
          <span class="text-[0.68rem] text-[#74747d]">Отдача</span><strong
            class="overflow-hidden text-ellipsis whitespace-nowrap text-[clamp(0.78rem,3.5vw,1.02rem)] font-bold"
            >{formatRate(status.stats.tx_bps)}</strong
          >
        </div>
      </article>
      <article
        class="grid min-h-28 min-w-0 grid-cols-[2.75rem_minmax(0,1fr)] items-center gap-x-3 rounded-[28px] border border-[#dedee1] bg-white p-4 shadow-sm"
      >
        <span
          class="grid size-11 place-items-center rounded-[14px] bg-[#f0f0f2]"
          ><Wifi size={19} /></span
        >
        <div class="flex min-w-0 flex-col justify-center gap-1">
          <span class="text-[0.68rem] text-[#74747d]">Устройства</span><strong
            class="overflow-hidden text-ellipsis whitespace-nowrap text-base font-bold"
            >{status.stats.wifi_clients}</strong
          >
        </div>
      </article>
      <article
        class="grid min-h-28 min-w-0 grid-cols-[2.75rem_minmax(0,1fr)] items-center gap-x-3 rounded-[28px] border border-[#dedee1] bg-white p-4 shadow-sm"
      >
        <span
          class="grid size-11 place-items-center rounded-[14px] bg-[#f0f0f2]"
          ><Cpu size={19} /></span
        >
        <div class="flex min-w-0 flex-col justify-center gap-1">
          <span class="text-[0.68rem] text-[#74747d]">Процессор</span><strong
            class="overflow-hidden text-ellipsis whitespace-nowrap text-base font-bold"
            >{status.stats.load_percent.toLocaleString("ru-RU", {
              maximumFractionDigits: 1,
            })}%</strong
          >
        </div>
      </article>
    </div>
  </div>

  <article
    class="min-w-0 overflow-hidden rounded-[28px] border border-[#dedee1] bg-white px-4 pb-3 pt-5 shadow-sm sm:px-6 sm:pt-6"
  >
    <div class="mb-4 flex min-w-0 items-start justify-between gap-3 px-1">
      <div class="min-w-0">
        <span
          class="text-xs font-bold tracking-[0.18em] text-[#74747d] uppercase"
          >Последние замеры</span
        >
        <h2 class="mt-1.5 text-xl font-bold tracking-[-0.035em]">
          Трафик сети
        </h2>
      </div>
      <a
        class="shrink-0 text-xs font-bold underline underline-offset-4"
        href={p("/analytics")}>Подробнее</a
      >
    </div>
    <MetricsChart
      history={status.history}
      kind="traffic"
      label="Краткий график скачивания и отдачи"
    />
  </article>

  <article
    class="grid min-w-0 overflow-hidden rounded-[28px] border border-[#dedee1] bg-white shadow-sm sm:grid-cols-3 sm:divide-x sm:divide-[#ececef]"
  >
    <div
      class="grid min-h-16 grid-cols-[1.5rem_1fr_auto] items-center gap-2.5 border-b border-[#ececef] px-5 sm:border-b-0"
    >
      <Cpu size={18} /><span class="text-xs text-[#74747d]">CPU</span><strong
        class="text-xs font-bold"
        >{status.stats.load_percent.toLocaleString("ru-RU", {
          maximumFractionDigits: 1,
        })}%</strong
      >
    </div>
    <div
      class="grid min-h-16 grid-cols-[1.5rem_1fr_auto] items-center gap-2.5 border-b border-[#ececef] px-5 sm:border-b-0"
    >
      <MemoryStick size={18} /><span class="text-xs text-[#74747d]">Память</span
      ><strong class="text-xs font-bold"
        >{status.stats.memory_percent.toLocaleString("ru-RU", {
          maximumFractionDigits: 1,
        })}%</strong
      >
    </div>
    <div
      class="grid min-h-16 grid-cols-[1.5rem_1fr_auto] items-center gap-2.5 px-5"
    >
      <Thermometer size={18} /><span class="text-xs text-[#74747d]"
        >Температура</span
      ><strong class="text-xs font-bold"
        >{status.stats.temperature_c == null
          ? "—"
          : `${status.stats.temperature_c.toLocaleString("ru-RU", { maximumFractionDigits: 1 })} °C`}</strong
      >
    </div>
  </article>
</section>
