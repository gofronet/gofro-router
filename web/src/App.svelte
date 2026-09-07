<script lang="ts">
  import { onMount } from "svelte";

  import { setAppContext } from "./app-context";
  import Login from "./pages/login.svelte";
  import Onboarding from "./pages/onboarding.svelte";
  import { Router } from "./router";
  import { RouterState } from "./stores/router-state.svelte";

  const app = new RouterState();
  setAppContext(app);

  onMount(() => {
    scrollTo(0, 0);
    void app.initializeAuth();
    return () => app.stop();
  });
</script>

{#if app.authLoading}
  <main class="grid min-h-dvh place-items-center bg-[#f5f5f5] text-[#09090b]" aria-live="polite"><div class="text-center"><span class="mx-auto block size-8 animate-spin rounded-full border-[3px] border-[#dedee1] border-t-[#09090b]"></span><strong class="mt-5 block">Проверяем доступ</strong></div></main>
{:else if app.authState === "authenticated"}
  {#if app.onboardingLoading && !app.onboarding}
    <main class="grid min-h-dvh place-items-center bg-[#f5f5f5] text-[#09090b]" aria-live="polite"><div class="text-center"><span class="mx-auto block size-8 animate-spin rounded-full border-[3px] border-[#dedee1] border-t-[#09090b]"></span><strong class="mt-5 block">Проверяем настройку</strong></div></main>
  {:else if app.onboarding?.step === "complete"}
    <Router />
  {:else if app.onboarding}
    <Onboarding />
  {:else}
    <main class="grid min-h-dvh place-items-center bg-[#f5f5f5] p-4 text-[#09090b]"><section class="max-w-md rounded-[28px] border border-[#dedee1] bg-white p-6 text-center"><h1 class="text-xl font-bold">Не удалось получить настройку</h1><p class="mt-2 text-sm text-[#74747d]">{app.actionError || "Проверьте подключение к роутеру."}</p><button class="mt-5 min-h-12 rounded-2xl bg-[#09090b] px-5 text-sm font-bold text-white" type="button" onclick={app.loadOnboarding}>Повторить</button></section></main>
  {/if}
{:else}
  <Login />
{/if}
