<script lang="ts">
  import type { Snippet } from "svelte";

  import ChevronRight from "lucide-svelte/icons/chevron-right";
  import House from "lucide-svelte/icons/house";
  import Shield from "lucide-svelte/icons/shield";
  import ListTree from "lucide-svelte/icons/list-tree";
  import Settings from "lucide-svelte/icons/settings";

  import { getAppContext } from "./app-context";
  import { isActive, p, route } from "./router";
  import OperationWarning from "./components/operation-warning.svelte";

  let { children }: { children: Snippet } = $props();

  const navigation = [
    { path: "/", label: "Обзор", icon: House, title: "Обзор" },
    { path: "/servers", label: "Серверы", icon: Shield, title: "Серверы" },
    { path: "/routing", label: "Правила", icon: ListTree, title: "Правила" },
    { path: "/system", label: "Панель", icon: Settings, title: "Панель" },
  ] as const;

  const app = getAppContext();
  const status = $derived(app.hasStatus ? app.status : null);
  const loading = $derived(app.loading);
  const pollError = $derived(app.pollError);
  const actionError = $derived(app.actionError);
  const current = $derived(isActive("/server-management") ? navigation[1] : navigation.find((item) => isActive(item.path)) ?? navigation[0]);
</script>

<svelte:head><meta name="theme-color" content="#f7f7f7" /></svelte:head>

<a class="skip" href="#content" onclick={(event) => { event.preventDefault(); document.getElementById("content")?.focus(); }}>К содержимому</a>
<aside class="sidebar" aria-label="Основная навигация">
  <a class="brand" href={p("/")}>Gofro<span>VPN</span></a>
  <nav class="nav-list">
    {#each navigation as item (item.path)}
      {@const Icon = item.icon}
      <a class:active={current.title === item.title} class="nav-link" href={p(item.path)} aria-current={current.title === item.title ? isActive(item.path) ? "page" : "true" : undefined}><Icon class="icon" /><span>{item.label}</span></a>
    {/each}
  </nav>
</aside>

<div class="workspace">
  <header class="topbar">
    <div class="breadcrumb"><span>Gofro VPN</span><ChevronRight class="icon" /><strong>{current.title}</strong></div>
    <a class="brand mobile-brand" href={p("/")}>Gofro<span>VPN</span></a>
  </header>
  <main class="app-main" id="content" tabindex="-1">
    <div class="page-heading"><h1>{current.title}</h1></div>
    {#if pollError && status}<div class="notice" role="status">Нет свежих данных. Показано последнее состояние: {pollError}</div>{/if}
    {#if actionError}<div class="notice error" role="alert"><strong>Операция требует внимания.</strong> {actionError} <button class="btn ghost" type="button" onclick={app.clearActionError}>Закрыть</button></div>{/if}
    {#if !(isActive("/server-management") && app.warningServerKey === route.search.server)}<OperationWarning />{/if}
    {#if status?.routing.degraded}<div class="notice error" role="alert">Не удалось восстановить маршрутизацию. Сохранённые правила могут не соответствовать действующим. Повторно сохраните правила.</div>{/if}
    {#if loading}
    <section class="grid min-h-[55vh] place-items-center text-center" aria-live="polite"><div><span class="loader"></span><p class="mt-5"><strong>Подключаемся к Gofro VPN</strong></p><p class="muted mt-2">Получаем состояние VPN</p></div></section>
    {:else if !status}
      <section class="panel mx-auto mt-8 max-w-xl p-8 text-center" role="alert"><h2>Устройство не отвечает</h2><p class="muted mt-2">{pollError || "Не удалось получить состояние контроллера."}</p><button class="btn primary mt-6" type="button" onclick={app.refresh}>Повторить</button></section>
    {:else}
      {@render children()}
    {/if}
  </main>
</div>

<nav class="mobile-nav" aria-label="Основная навигация">
  {#each navigation as item (item.path)}
    {@const Icon = item.icon}
    <a class:active={current.title === item.title} href={p(item.path)} aria-current={current.title === item.title ? isActive(item.path) ? "page" : "true" : undefined}><Icon class="icon" /><span>{item.label}</span></a>
  {/each}
</nav>
