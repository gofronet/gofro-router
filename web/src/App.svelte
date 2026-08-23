<script lang="ts">
  import { onMount } from 'svelte';
  import ChartNoAxesCombined from 'lucide-svelte/icons/chart-no-axes-combined';
  import House from 'lucide-svelte/icons/house';
  import ServerCog from 'lucide-svelte/icons/server-cog';
  import WifiCog from 'lucide-svelte/icons/wifi-cog';
  import Analytics from './Analytics.svelte';
  import Home from './Home.svelte';
  import Servers from './Servers.svelte';
  import WifiSettings from './WifiSettings.svelte';
  import type { Mutate, Status, UpdateState, UpdateStatus } from './types';

  type Screen = 'home' | 'analytics' | 'servers' | 'wifi';

  const screens: Screen[] = ['home', 'analytics', 'servers', 'wifi'];
  const navigation = [
    { id: 'home' as const, label: 'Главная', icon: House },
    { id: 'analytics' as const, label: 'Аналитика', icon: ChartNoAxesCombined },
    { id: 'servers' as const, label: 'Серверы', icon: ServerCog },
    { id: 'wifi' as const, label: 'Wi-Fi', icon: WifiCog }
  ];

  let status = $state<Status | null>(null);
  let loading = $state(true);
  let pollError = $state('');
  let actionError = $state('');
  let mutation = $state<string | null>(null);
  let screen = $state<Screen>('home');
  let reconnectSsid = $state<string | null>(null);
  let updateStatus = $state<UpdateStatus | null>(null);
  let updateError = $state('');
  let updatePollError = $state('');
  let updateAction = $state<'check' | 'start' | null>(null);
  let pollInFlight = false;
  let updaterPollInFlight = false;
  let statusVersion = 0;
  let updaterVersion = 0;
  let observedInstalledVersion: string | null = null;

  const activeUpdateStates: UpdateState[] = ['checking', 'downloading', 'installing'];
  const updateActive = $derived(updateStatus !== null && activeUpdateStates.includes(updateStatus.state));
  const updaterError = $derived(updateError || updatePollError);
  const busy = $derived(mutation !== null || updateActive || updateAction !== null);

  function message(error: unknown) {
    return error instanceof Error ? error.message : 'Неизвестная ошибка';
  }

  function screenFromHash(): Screen {
    const hash = location.hash.slice(1);
    return screens.includes(hash as Screen) ? hash as Screen : 'home';
  }

  async function parseResponse<T>(response: Response): Promise<T> {
    const text = await response.text();
    let body: unknown = null;
    if (text) {
      try {
        body = JSON.parse(text);
      } catch {
        throw new Error(response.ok ? 'Сервер вернул некорректный JSON' : `Ошибка HTTP ${response.status}`);
      }
    }
    if (!response.ok) {
      const error = body && typeof body === 'object' && 'error' in body && typeof body.error === 'string'
        ? body.error
        : `Ошибка HTTP ${response.status}`;
      throw new Error(error);
    }
    if (body === null) throw new Error('Сервер вернул пустой ответ');
    return body as T;
  }

  async function api<T>(path: string, init: RequestInit = {}): Promise<T> {
    return parseResponse(await fetch(path, { ...init, headers: { 'Content-Type': 'application/json' } }));
  }

  async function updaterApi<T>(path: string, method = 'GET'): Promise<T> {
    return parseResponse(await fetch(`http://${location.hostname}:8080${path}`, { method, cache: 'no-store' }));
  }

  async function refresh() {
    if (pollInFlight || mutation || reconnectSsid) return;
    pollInFlight = true;
    const version = statusVersion;
    try {
      const nextStatus = await api<Status>('/api/status');
      if (version === statusVersion) {
        observedInstalledVersion ??= nextStatus.version;
        status = nextStatus;
        pollError = '';
      }
    } catch (error) {
      if (version === statusVersion) pollError = message(error);
    } finally {
      loading = false;
      pollInFlight = false;
    }
  }

  async function refreshUpdate() {
    if (updaterPollInFlight || updateAction) return;
    updaterPollInFlight = true;
    const version = updaterVersion;
    try {
      const next = await updaterApi<UpdateStatus>('/api/status');
      if (version === updaterVersion) {
        updateStatus = next;
        updateError = '';
        updatePollError = '';
        if (next.state === 'success' && observedInstalledVersion && observedInstalledVersion !== next.installed_version) location.reload();
        observedInstalledVersion ??= next.installed_version;
      }
    } catch (error) {
      if (version === updaterVersion) updatePollError = message(error);
    } finally {
      updaterPollInFlight = false;
    }
  }

  async function requestUpdate(kind: 'check' | 'start', path: string) {
    if (updateAction || updateActive) return false;
    updaterVersion++;
    updateAction = kind;
    updateError = '';
    try {
      updateStatus = await updaterApi<UpdateStatus>(path, 'POST');
      return true;
    } catch (error) {
      updateError = message(error);
      return false;
    } finally {
      updateAction = null;
    }
  }

  async function checkUpdate() {
    await requestUpdate('check', '/api/check');
  }

  async function startUpdate() {
    if (!confirm('Во время установки VPN и панель будут недоступны несколько секунд. Продолжить?')) return;
    await requestUpdate('start', '/api/start');
  }

  function updateLabel() {
    if (!updateStatus) return '';
    return {
      idle: updateStatus.version ? 'Установлена актуальная версия' : 'Обновления ещё не проверялись',
      checking: 'Проверяем наличие обновлений',
      available: `Доступна версия ${updateStatus.version ?? ''}`,
      downloading: 'Скачиваем и проверяем пакет',
      installing: 'Устанавливаем обновление',
      success: `Версия ${updateStatus.version ?? ''} установлена`,
      error: updateStatus.message || 'Обновление не выполнено'
    }[updateStatus.state];
  }

  const mutate: Mutate = async (kind, path, init) => {
    if (busy) return false;
    statusVersion++;
    mutation = kind;
    actionError = '';
    try {
      status = await api<Status>(path, init);
      pollError = '';
      return true;
    } catch (error) {
      actionError = message(error);
      return false;
    } finally {
      mutation = null;
    }
  };

  async function setMode(vpnEnabled: boolean) {
    if (status?.vpn_enabled === vpnEnabled && (!vpnEnabled || status.tunnel_active)) return;
    await mutate('mode', '/api/mode', {
      method: 'POST',
      body: JSON.stringify({ vpn_enabled: vpnEnabled })
    });
  }

  async function saveAp(ssid: string, password: string) {
    const saved = await mutate('ap', '/api/ap', {
      method: 'POST',
      body: JSON.stringify({ ssid, password })
    });
    if (saved) reconnectSsid = ssid;
    return saved;
  }

  async function resumePolling() {
    reconnectSsid = null;
    await refresh();
  }

  onMount(() => {
    screen = screenFromHash();
    if (!location.hash) history.replaceState(null, '', `${location.pathname}${location.search}#home`);
    const onHashChange = () => {
      screen = screenFromHash();
      scrollTo({ top: 0, behavior: 'instant' });
    };
    addEventListener('hashchange', onHashChange);
    scrollTo(0, 0);
    void refresh();
    void refreshUpdate();
    const interval = window.setInterval(refresh, 2000);
    const updaterInterval = window.setInterval(refreshUpdate, 1000);
    return () => {
      removeEventListener('hashchange', onHashChange);
      window.clearInterval(interval);
      window.clearInterval(updaterInterval);
    };
  });
</script>

<svelte:head>
  <meta name="theme-color" content="#fafafa" />
</svelte:head>

<div class="min-h-dvh bg-[#f5f5f5] font-sans text-[#09090b] lg:grid lg:grid-cols-[250px_minmax(0,1fr)]">
  <aside class="sticky top-0 hidden h-dvh flex-col border-r border-[#dedee1] bg-white/80 px-5 py-8 backdrop-blur-xl lg:flex" aria-label="Основная навигация">
    <a class="flex min-h-12 items-center gap-3 text-xl font-bold tracking-[-0.045em] no-underline" href="#home" aria-label="GofroWiFi, главная">
      <span class="flex size-12 items-end justify-center gap-[3px] rounded-2xl bg-[#09090b] px-2.5 py-3 shadow-lg shadow-black/10"><i class="h-2 w-1 rounded-sm bg-white"></i><i class="h-4 w-1 rounded-sm bg-white"></i><i class="h-6 w-1 rounded-sm bg-white"></i></span>
      <span class="flex flex-wrap"><strong class="font-extrabold">Gofro</strong>WiFi<small class="mt-1 block basis-full text-[0.6rem] font-medium tracking-[0.08em] text-[#74747d]">network appliance</small></span>
    </a>
    <nav class="mt-14 grid gap-2">
      {#each navigation as item}
        {@const Icon = item.icon}
        <a href={`#${item.id}`} class={`flex min-h-14 items-center gap-3 rounded-[18px] px-4 text-sm font-semibold no-underline transition-colors ${screen === item.id ? 'bg-[#09090b] text-white' : 'text-[#74747d] hover:bg-[#f0f0f2] hover:text-[#09090b]'}`} aria-current={screen === item.id ? 'page' : undefined}>
          <Icon size={21} strokeWidth={1.8} />
          <span>{item.label}</span>
        </a>
      {/each}
    </nav>
    <div class="mt-auto grid gap-4 border-t border-[#ececef] px-3 pt-5">
      <span class={`flex items-center gap-2 text-xs font-bold ${status?.tunnel_active ? 'text-[#09090b]' : 'text-[#74747d]'}`}><i class="size-2 bg-current"></i>{status?.tunnel_active ? 'VPN защищен' : status?.vpn_enabled === false ? 'Режим DIRECT' : 'VPN недоступен'}</span>
      <a class="text-xs text-[#74747d] no-underline" href="http://gofrowifi.net">gofrowifi.net</a>
    </div>
  </aside>

  <header class="sticky top-0 z-40 flex min-h-[76px] items-center justify-between border-b border-[#dedee1]/80 bg-[#fafafa]/90 px-4 pb-2 pt-[max(0.75rem,env(safe-area-inset-top))] backdrop-blur-xl lg:hidden">
    <a class="flex min-h-12 items-center gap-2.5 text-lg font-bold tracking-[-0.045em] no-underline" href="#home" aria-label="GofroWiFi, главная">
      <span class="flex size-11 items-end justify-center gap-[3px] rounded-[15px] bg-[#09090b] px-2.5 py-2.5 shadow-lg shadow-black/10"><i class="h-2 w-1 rounded-sm bg-white"></i><i class="h-3.5 w-1 rounded-sm bg-white"></i><i class="h-5 w-1 rounded-sm bg-white"></i></span>
      <span><strong class="font-extrabold">Gofro</strong>WiFi</span>
    </a>
    <span class={`flex items-center gap-2 text-xs font-bold ${status?.tunnel_active ? 'text-[#09090b]' : 'text-[#74747d]'}`} aria-label="Состояние подключения"><i class="size-2 bg-current"></i>{status?.vpn_enabled === false ? 'DIRECT' : 'VPN'}</span>
  </header>

  <main class="mx-auto w-full min-w-0 max-w-[1380px] px-4 pb-[calc(5.75rem+env(safe-area-inset-bottom))] pt-6 sm:px-7 lg:px-[clamp(2.25rem,4vw,4.25rem)] lg:py-12">
    {#if updateActive}
      <div class="mb-4 flex items-center gap-3 rounded-2xl border border-[#c9d7eb] bg-[#f3f7fc] px-4 py-3 text-xs leading-relaxed text-[#344b6a]" role="status" aria-live="polite"><span class="size-2 shrink-0 animate-pulse rounded-full bg-[#344b6a]"></span><span class="min-w-0 flex-1">{updatePollError ? 'Связь с сервисом обновлений потеряна. Обновление может продолжаться в фоне.' : `${updateLabel()}. Не выключайте устройство.`}</span>{#if updatePollError}<button class="min-h-10 shrink-0 rounded-xl border border-[#344b6a] bg-transparent px-3 font-bold" type="button" onclick={() => void refreshUpdate()}>Переподключиться</button>{/if}</div>
    {/if}
    {#if pollError && status}
      <div class="mb-4 flex items-center justify-between gap-3 rounded-2xl border border-[#dfc99e] bg-[#fffaf0] px-4 py-3 text-xs leading-relaxed text-[#6d5730]" role="status"><span>{updateActive ? 'Контроллер перезапускается, прогресс обновления продолжает работать.' : `Нет свежих данных. Показано последнее состояние: ${pollError}`}</span></div>
    {/if}
    {#if actionError}
      <div class="mb-4 flex items-center justify-between gap-3 rounded-2xl border border-red-200 bg-red-50 px-4 py-3 text-xs leading-relaxed text-red-700" role="alert"><span><strong>Операция не выполнена.</strong> {actionError}</span><button class="min-h-11 border-0 bg-transparent font-bold" type="button" aria-label="Закрыть сообщение об ошибке" onclick={() => actionError = ''}>Закрыть</button></div>
    {/if}

    {#if loading}
      <section class="flex min-h-[55vh] flex-col items-center justify-center text-center" aria-live="polite"><span class="size-8 animate-spin rounded-full border-[3px] border-[#dedee1] border-t-[#09090b]"></span><strong class="mt-5">Подключаемся к GofroWiFi</strong><p class="mt-2 text-sm text-[#74747d]">Получаем состояние сети</p></section>
    {:else if !status && updateActive}
      <section class="flex min-h-[55vh] flex-col items-center justify-center text-center" aria-live="polite"><span class="size-8 animate-spin rounded-full border-[3px] border-[#dedee1] border-t-[#09090b]"></span><strong class="mt-5">{updateLabel()}</strong><p class="mt-2 text-sm text-[#74747d]">Панель подключится снова после перезапуска контроллера.</p></section>
    {:else if !status}
      <section class="mx-auto mt-8 flex min-h-[55vh] max-w-xl flex-col items-center justify-center rounded-[28px] border border-[#dedee1] bg-white p-8 text-center shadow-sm" role="alert"><span class="mb-5 flex size-14 items-end justify-center gap-[3px] rounded-[18px] bg-[#09090b] px-3 py-3"><i class="h-2 w-1 rounded-sm bg-white"></i><i class="h-4 w-1 rounded-sm bg-white"></i><i class="h-6 w-1 rounded-sm bg-white"></i></span><h1 class="m-0 text-2xl font-bold">Устройство не отвечает</h1><p class="mt-2 text-sm text-[#74747d]">{pollError || 'Не удалось получить состояние контроллера.'}</p><button class="mt-6 min-h-13 rounded-2xl border border-[#09090b] bg-[#09090b] px-5 font-bold text-white" type="button" onclick={refresh}>Повторить</button></section>
    {:else if screen === 'home'}
      <Home {status} {busy} {mutation} {mutate} />
    {:else if screen === 'analytics'}
      <Analytics {status} />
    {:else if screen === 'servers'}
      <Servers {status} {busy} {mutation} {mutate} />
    {:else}
      <WifiSettings {status} {busy} {mutation} {reconnectSsid} {updateStatus} updateError={updaterError} {updateAction} {setMode} {saveAp} {resumePolling} {checkUpdate} {startUpdate} />
    {/if}
  </main>

  <nav class="fixed inset-x-0 bottom-0 z-50 grid min-h-[calc(4.75rem+env(safe-area-inset-bottom))] grid-cols-4 border-t border-[#dedee1] bg-white/95 px-1.5 pt-1.5 pb-[env(safe-area-inset-bottom)] shadow-[0_-8px_24px_rgba(0,0,0,0.04)] backdrop-blur-xl lg:hidden" aria-label="Основная навигация">
    {#each navigation as item}
      {@const Icon = item.icon}
      <a href={`#${item.id}`} class={`flex min-w-0 flex-col items-center justify-center gap-1 rounded-[18px] text-[clamp(0.6rem,2.6vw,0.68rem)] font-semibold no-underline ${screen === item.id ? 'bg-[#09090b] text-white' : 'text-[#74747d]'}`} aria-current={screen === item.id ? 'page' : undefined}>
        <Icon size={19} strokeWidth={1.9} />
        <span>{item.label}</span>
      </a>
    {/each}
  </nav>
</div>
