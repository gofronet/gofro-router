<script lang="ts">
  import { onMount } from "svelte";

  import { setAppContext } from "./app-context";
  import Login from "./pages/login.svelte";
  import Onboarding from "./pages/onboarding.svelte";
  import { Router } from "./router";
  import { RouterState } from "./stores/router-state.svelte";
  import { initializeTheme } from "./stores/theme.svelte";

  const app = new RouterState();
  setAppContext(app);

  onMount(() => {
    initializeTheme();
    scrollTo(0, 0);
    void app.initializeAuth();
    return () => app.stop();
  });
</script>

{#if app.authLoading}
  <main class="access-page" aria-live="polite"><div class="text-center"><span class="loader"></span><p class="muted mt-5">Проверяем доступ</p></div></main>
{:else if app.authState === "authenticated"}
  {#if app.onboardingLoading && !app.onboarding}
    <main class="access-page" aria-live="polite"><div class="text-center"><span class="loader"></span><p class="muted mt-5">Проверяем настройку</p></div></main>
  {:else if app.onboarding?.step === "complete"}
    <Router />
  {:else if app.onboarding}
    <Onboarding />
  {:else}
    <main class="access-page"><section class="access-card"><h1>Не удалось получить настройку</h1><p class="access-note">{app.actionError || "Проверьте подключение к роутеру."}</p><button class="btn primary mt-5" type="button" onclick={app.loadOnboarding}>Повторить</button></section></main>
  {/if}
{:else}
  <Login />
{/if}
