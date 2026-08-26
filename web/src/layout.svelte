<script lang="ts">
  import type { Snippet } from "svelte";

  import ChartNoAxesCombined from "lucide-svelte/icons/chart-no-axes-combined";
  import House from "lucide-svelte/icons/house";
  import Route from "lucide-svelte/icons/route";
  import ServerCog from "lucide-svelte/icons/server-cog";
  import WifiCog from "lucide-svelte/icons/wifi-cog";

  import { getAppContext } from "./app-context";
  import { isActive, p } from "./router";

  let { children }: { children: Snippet } = $props();

  const navigation = [
    { path: "/" as const, label: "Главная", icon: House },
    {
      path: "/analytics" as const,
      label: "Аналитика",
      icon: ChartNoAxesCombined,
    },
    { path: "/servers" as const, label: "Серверы", icon: ServerCog },
    { path: "/routing" as const, label: "Маршруты", icon: Route },
    { path: "/wifi" as const, label: "Настройки", icon: WifiCog },
  ];

  const app = getAppContext();
  const status = $derived(app.hasStatus ? app.status : null);
  const loading = $derived(app.loading);
  const pollError = $derived(app.pollError);
  const actionError = $derived(app.actionError);
</script>

<svelte:head>
  <meta name="theme-color" content="#fafafa" />
</svelte:head>

<div
  class="min-h-dvh bg-[#f5f5f5] font-sans text-[#09090b] lg:pl-[250px]"
>
  <aside
    class="fixed inset-y-0 left-0 hidden w-[250px] flex-col overflow-y-auto border-r border-[#dedee1] bg-white/80 px-5 py-8 backdrop-blur-xl lg:flex"
    aria-label="Основная навигация"
  >
    <a
      class="flex min-h-12 items-center gap-3 text-xl font-bold tracking-[-0.045em] no-underline"
      href={p("/")}
      aria-label="Gofro Router, главная"
    >
      <span
        class="flex size-12 items-end justify-center gap-0.75 rounded-2xl bg-[#09090b] px-2.5 py-3 shadow-lg shadow-black/10"
        ><i class="h-2 w-1 rounded-sm bg-white"></i><i
          class="h-4 w-1 rounded-sm bg-white"
        ></i><i class="h-6 w-1 rounded-sm bg-white"></i></span
      >
      <span class="flex flex-wrap"
        ><strong class="font-extrabold">Gofro</strong> Router<small
          class="mt-1 block basis-full text-[0.6rem] font-medium tracking-[0.08em] text-[#74747d]"
          >network appliance</small
        ></span
      >
    </a>
    <nav class="mt-14 grid gap-2">
      {#each navigation as item}
        {@const Icon = item.icon}
        <a
          href={p(item.path)}
          class={`flex min-h-14 items-center gap-3 rounded-[18px] px-4 text-sm font-semibold no-underline transition-colors ${isActive(item.path) ? "bg-[#09090b] text-white" : "text-[#74747d] hover:bg-[#f0f0f2] hover:text-[#09090b]"}`}
          aria-current={isActive(item.path) ? "page" : undefined}
        >
          <Icon size={21} strokeWidth={1.8} />
          <span>{item.label}</span>
        </a>
      {/each}
    </nav>
    <div class="mt-auto grid gap-4 border-t border-[#ececef] px-3 pt-5">
      <span
        class={`flex items-center gap-2 text-xs font-bold ${status?.tunnel_active ? "text-[#09090b]" : "text-[#74747d]"}`}
        ><i class="size-2 bg-current"></i>{status?.tunnel_active
          ? "VPN защищен"
          : status?.vpn_enabled === false
            ? "Режим DIRECT"
            : "VPN недоступен"}</span
      >
      <a class="text-xs text-[#74747d] no-underline" href="http://gofrowifi.net:8080"
        >gofrowifi.net</a
      >
    </div>
  </aside>

  <header
    class="sticky top-0 z-40 flex min-h-19 items-center justify-between border-b border-[#dedee1]/80 bg-[#fafafa]/90 px-4 pb-2 pt-[max(0.75rem,env(safe-area-inset-top))] backdrop-blur-xl lg:hidden"
  >
    <a
      class="flex min-h-12 items-center gap-2.5 text-lg font-bold tracking-[-0.045em] no-underline"
      href={p("/")}
      aria-label="Gofro Router, главная"
    >
      <span
        class="flex size-11 items-end justify-center gap-0.75 rounded-[15px] bg-[#09090b] px-2.5 py-2.5 shadow-lg shadow-black/10"
        ><i class="h-2 w-1 rounded-sm bg-white"></i><i
          class="h-3.5 w-1 rounded-sm bg-white"
        ></i><i class="h-5 w-1 rounded-sm bg-white"></i></span
      >
      <span><strong class="font-extrabold">Gofro</strong> Router</span>
    </a>
    <span
      class={`flex items-center gap-2 text-xs font-bold ${status?.tunnel_active ? "text-[#09090b]" : "text-[#74747d]"}`}
      aria-label="Состояние подключения"
      ><i class="size-2 bg-current"></i>{status?.vpn_enabled === false
        ? "DIRECT"
        : "VPN"}</span
    >
  </header>

  <main
    class="mx-auto w-full min-w-0 max-w-345 px-4 pb-[calc(5.75rem+env(safe-area-inset-bottom))] pt-6 sm:px-7 lg:px-[clamp(2.25rem,4vw,4.25rem)] lg:py-12"
  >
    {#if pollError && status}
      <div
        class="mb-4 flex items-center justify-between gap-3 rounded-2xl border border-[#dfc99e] bg-[#fffaf0] px-4 py-3 text-xs leading-relaxed text-[#6d5730]"
        role="status"
      >
        <span>Нет свежих данных. Показано последнее состояние: {pollError}</span>
      </div>
    {/if}
    {#if actionError}
      <div
        class="mb-4 flex items-center justify-between gap-3 rounded-2xl border border-red-200 bg-red-50 px-4 py-3 text-xs leading-relaxed text-red-700"
        role="alert"
      >
        <span><strong>Операция не выполнена.</strong> {actionError}</span
        ><button
          class="min-h-11 border-0 bg-transparent font-bold"
          type="button"
          aria-label="Закрыть сообщение об ошибке"
          onclick={app.clearActionError}>Закрыть</button
        >
      </div>
    {/if}

    {#if loading}
      <section
        class="flex min-h-[55vh] flex-col items-center justify-center text-center"
        aria-live="polite"
      >
        <span
          class="size-8 animate-spin rounded-full border-[3px] border-[#dedee1] border-t-[#09090b]"
        ></span><strong class="mt-5">Подключаемся к Gofro Router</strong>
        <p class="mt-2 text-sm text-[#74747d]">Получаем состояние сети</p>
      </section>
    {:else if !status}
      <section
        class="mx-auto mt-8 flex min-h-[55vh] max-w-xl flex-col items-center justify-center rounded-[28px] border border-[#dedee1] bg-white p-8 text-center shadow-sm"
        role="alert"
      >
        <span
          class="mb-5 flex size-14 items-end justify-center gap-0.75 rounded-[18px] bg-[#09090b] px-3 py-3"
          ><i class="h-2 w-1 rounded-sm bg-white"></i><i
            class="h-4 w-1 rounded-sm bg-white"
          ></i><i class="h-6 w-1 rounded-sm bg-white"></i></span
        >
        <h1 class="m-0 text-2xl font-bold">Устройство не отвечает</h1>
        <p class="mt-2 text-sm text-[#74747d]">
          {pollError || "Не удалось получить состояние контроллера."}
        </p>
        <button
          class="mt-6 min-h-13 rounded-2xl border border-[#09090b] bg-[#09090b] px-5 font-bold text-white"
          type="button"
          onclick={app.refresh}>Повторить</button
        >
      </section>
    {:else}
      {@render children()}
    {/if}
  </main>

  <nav
    class="fixed inset-x-0 bottom-0 z-50 grid min-h-[calc(4.75rem+env(safe-area-inset-bottom))] grid-cols-5 border-t border-[#dedee1] bg-white/95 px-1.5 pt-1.5 pb-[env(safe-area-inset-bottom)] shadow-[0_-8px_24px_rgba(0,0,0,0.04)] backdrop-blur-xl lg:hidden"
    aria-label="Основная навигация"
  >
    {#each navigation as item}
      {@const Icon = item.icon}
      <a
        href={p(item.path)}
        class={`flex min-w-0 flex-col items-center justify-center gap-1 rounded-[18px] text-[clamp(0.54rem,2.35vw,0.66rem)] font-semibold no-underline ${isActive(item.path) ? "bg-[#09090b] text-white" : "text-[#74747d]"}`}
        aria-current={isActive(item.path) ? "page" : undefined}
      >
        <Icon size={19} strokeWidth={1.9} />
        <span>{item.label}</span>
      </a>
    {/each}
  </nav>
</div>
