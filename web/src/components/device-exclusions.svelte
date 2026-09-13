<script lang="ts">
  import { onMount } from "svelte";
  import { macSchema } from "../api/schemas";
  import { getAppContext } from "../app-context";

  const app = getAppContext();
  let mac = $state("");
  let error = $state("");
  const saved = $derived(app.status.device_exclusions);
  const devices = $derived.by(() => {
    const inventory = new Map(app.lanDevices.devices.map(device => [device.mac, device]));
    for (const mac of saved) if (!inventory.has(mac)) inventory.set(mac, { mac, name: null, addresses: [] });
    return [...inventory.values()];
  });
  const disabled = $derived(app.busy || app.statusUncertain || Boolean(app.pollError));
  onMount(() => { void app.refreshLanDevices(); });

  async function add(event: SubmitEvent) {
    event.preventDefault();
    const parsed = macSchema.safeParse(mac);
    if (!parsed.success) { error = parsed.error.issues[0].message; return; }
    error = "";
    if (await app.setDeviceExcluded({ mac: parsed.data, excluded: true })) mac = "";
  }
</script>

<section class="panel" aria-labelledby="device-exclusions-title">
  <div class="panel-head"><h2 id="device-exclusions-title">Устройства напрямую <span class="count">{saved.length}/256</span></h2></div>
  <div class="panel-body">
    <p>Включите переключатель, чтобы весь интернет устройства шёл напрямую.</p>
    <button class="btn ghost" type="button" disabled={app.inventoryLoading} onclick={() => app.refreshLanDevices()}>{app.inventoryLoading ? "Ищем устройства…" : "Обновить список устройств"}</button>
    {#if app.lanDevices.discovery !== "complete"}
      <p class="notice" role="status">{app.lanDevices.discovery === "partial" ? "Список устройств неполный." : "Список устройств недоступен."} Можно добавить MAC вручную.</p>
    {/if}
    {#if app.inventoryError}<p class="small">{app.inventoryError}</p>{/if}
    {#each devices as device (device.mac)}
      {@const excluded = saved.includes(device.mac)}
      <div class="flex items-center gap-3 py-3">
        <button class="switch" type="button" role="switch" aria-checked={excluded} aria-label={`Напрямую: ${device.name || device.mac}`} disabled={disabled || (!excluded && saved.length >= 256)} onclick={() => app.setDeviceExcluded({ mac: device.mac, excluded: !excluded })}><span class="switch-track"></span></button>
        <div class="min-w-0 break-words"><strong>{device.name || "Устройство без имени"}</strong><p>{device.addresses.join(", ") || "Нет текущего IP"}</p><p class="small">{device.mac}{#if excluded} · Сохранено{/if}</p></div>
      </div>
    {/each}
    <form onsubmit={add}>
      <label class="field">MAC устройства<input bind:value={mac} placeholder="02:ab:cd:ef:01:23" autocapitalize="off" spellcheck="false" required aria-invalid={Boolean(error)} aria-describedby={error ? "device-mac-error" : undefined} /></label>
      {#if error}<p id="device-mac-error" class="error" role="alert">{error}</p>{/if}
      <button class="btn" disabled={disabled || saved.length >= 256}>Добавить напрямую</button>
      {#if saved.length >= 256}<p class="notice">Достигнут лимит 256 устройств. Можно отключить существующее исключение.</p>{/if}
    </form>
    {#if app.pollError}<button class="btn ghost" type="button" disabled={app.busy} onclick={() => app.refresh()}>Обновить состояние перед изменением</button>{/if}
  </div>
</section>
