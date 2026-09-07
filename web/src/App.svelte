<script lang="ts">
  import { onMount } from "svelte";

  import { setAppContext } from "./app-context";
  import Login from "./pages/login.svelte";
  import { Router } from "./router";
  import { RouterState } from "./stores/router-state.svelte";

  const app = new RouterState();
  setAppContext(app);

  onMount(() => {
    scrollTo(0, 0);
    void app.initializeAuth();
    return () => app.stopPolling();
  });
</script>

{#if app.authLoading}
  <main class="grid min-h-dvh place-items-center bg-[#f5f5f5] text-[#09090b]" aria-live="polite"><div class="text-center"><span class="mx-auto block size-8 animate-spin rounded-full border-[3px] border-[#dedee1] border-t-[#09090b]"></span><strong class="mt-5 block">Проверяем доступ</strong></div></main>
{:else if app.authState === "authenticated"}
  <Router />
{:else}
  <Login />
{/if}
