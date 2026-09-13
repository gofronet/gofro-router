<script lang="ts">
  import { getAppContext } from "../app-context";
  import { serverManagementPath } from "../router";

  let { onrefresh }: { onrefresh?: () => Promise<void> } = $props();
  const app = getAppContext();
  let refreshing = $state(false);
  async function refresh() {
    if (refreshing || app.busy) return;
    refreshing = true;
    try { await (app.statusUncertain ? app.refresh : onrefresh ?? app.refresh)(); }
    finally { refreshing = false; }
  }
</script>

{#if app.actionWarning || app.statusUncertain}
  <div class="notice" role="status">
    {app.actionWarning || "Показано последнее известное состояние. После изменения оно не подтверждено. Обновите состояние перед переключением VPN или выбором сервера."}
    {#if app.warningServerKey && !app.statusUncertain && !onrefresh}
      <a class="btn ghost" href={serverManagementPath(app.warningServerKey)}>Проверить состояние сервера</a>
    {:else}
      <button class="btn ghost" type="button" disabled={app.busy || refreshing} onclick={refresh}>{refreshing ? "Обновляем состояние…" : "Обновить состояние"}</button>
    {/if}
  </div>
{/if}
