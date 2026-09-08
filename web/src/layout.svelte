<script lang="ts">
  import type { Snippet } from "svelte";

  import ChevronRight from "lucide-svelte/icons/chevron-right";
  import House from "lucide-svelte/icons/house";
  import RouterIcon from "lucide-svelte/icons/router";
  import Shield from "lucide-svelte/icons/shield";
  import Wifi from "lucide-svelte/icons/wifi";

  import { getAppContext } from "./app-context";
  import { isActive, p } from "./router";

  let { children }: { children: Snippet } = $props();

  const navigation = [
    { path: "/", label: "Обзор", icon: House, title: "Обзор" },
    { path: "/servers", label: "VPN", icon: Shield, title: "VPN" },
    { path: "/devices", label: "Сеть", icon: Wifi, title: "Сеть" },
    { path: "/system", label: "Роутер", icon: RouterIcon, title: "Роутер" },
  ] as const;
  const sections = [
    { title: "VPN", items: [{ path: "/servers", label: "Серверы" }, { path: "/routing", label: "Правила" }] },
    { title: "Сеть", items: [{ path: "/devices", label: "Устройства" }, { path: "/wifi", label: "Wi-Fi" }] },
    { title: "Роутер", items: [{ path: "/system", label: "Система" }, { path: "/analytics", label: "Диагностика" }] },
  ] as const;

  const app = getAppContext();
  const status = $derived(app.hasStatus ? app.status : null);
  const loading = $derived(app.loading);
  const pollError = $derived(app.pollError);
  const actionError = $derived(app.actionError);
  const current = $derived(navigation.find((item) => isActive(item.path)) ?? navigation.find((item) => sections.some((section) => section.title === item.title && section.items.some((tab) => isActive(tab.path)))) ?? navigation[0]);
  const tabs = $derived(sections.find((section) => section.title === current.title)?.items ?? []);
</script>

<svelte:head><meta name="theme-color" content="#f7f7f7" /></svelte:head>

<a class="skip" href="#content" onclick={(event) => { event.preventDefault(); document.getElementById("content")?.focus(); }}>К содержимому</a>
<aside class="sidebar" aria-label="Основная навигация">
  <a class="brand" href={p("/")}>Gofro<span>Router</span></a>
  <nav class="nav-list">
    {#each navigation as item (item.path)}
      {@const Icon = item.icon}
      <a class:active={current.title === item.title} class="nav-link" href={p(item.path)} aria-current={current.title === item.title ? isActive(item.path) ? "page" : "true" : undefined}><Icon class="icon" /><span>{item.label}</span></a>
    {/each}
  </nav>
  <div class="side-bottom"><div class="router-label"><RouterIcon class="icon" /><div><div class="small">Домашний роутер</div><div class="mono muted">{status?.ap.address || "10.203.1.1"}</div></div></div></div>
</aside>

<div class="workspace">
  <header class="topbar">
    <div class="breadcrumb"><span>Домашний роутер</span><ChevronRight class="icon" /><strong>{current.title}</strong></div>
    <a class="brand mobile-brand" href={p("/")}>Gofro<span>Router</span></a>
  </header>
  <main class="app-main" id="content" tabindex="-1">
    <div class="page-heading"><h1>{current.title}</h1></div>
    {#if tabs.length}<nav class="tabs" aria-label={`Разделы ${current.title}`}>{#each tabs as tab (tab.path)}<a href={p(tab.path)} aria-current={isActive(tab.path) ? "page" : undefined}>{tab.label}</a>{/each}</nav>{/if}
    {#if pollError && status}<div class="notice" role="status">Нет свежих данных. Показано последнее состояние: {pollError}</div>{/if}
    {#if actionError}<div class="notice error" role="alert"><strong>Операция не выполнена.</strong> {actionError} <button class="btn ghost" type="button" onclick={app.clearActionError}>Закрыть</button></div>{/if}
    {#if status?.routing.degraded}<div class="notice error" role="alert">Не удалось восстановить маршрутизацию. Сохранённые правила могут не соответствовать действующим. Перезагрузите роутер или повторно сохраните правила.</div>{/if}
    {#if loading}
      <section class="grid min-h-[55vh] place-items-center text-center" aria-live="polite"><div><span class="loader"></span><p class="mt-5"><strong>Подключаемся к Gofro Router</strong></p><p class="muted mt-2">Получаем состояние сети</p></div></section>
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
