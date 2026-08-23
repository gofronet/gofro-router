<script lang="ts">
  import { onMount } from "svelte";

  import { setAppContext } from "./app-context";
  import { Router } from "./router";
  import { RouterState } from "./stores/router-state.svelte";

  const app = new RouterState();
  setAppContext(app);

  onMount(() => {
    scrollTo(0, 0);
    app.startPolling();
    return () => app.stopPolling();
  });
</script>

<Router />
