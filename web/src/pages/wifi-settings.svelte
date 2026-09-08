<script lang="ts">
  import { getAppContext } from "../app-context";
  import PasswordInput from "../components/password-input.svelte";
  import SharedDialog from "../components/dialog.svelte";
  import type { WifiBand } from "../domain/models";

  const app = getAppContext();
  const status = $derived(app.status);
  const busy = $derived(app.busy);
  const reconnectSsid = $derived(app.reconnectSsid);
  let selectedBand = $state<WifiBand | undefined>(undefined);
  let ssid = $state("");
  let password = $state("");
  let error = $state("");
  let confirmSave = $state(false);
  let confirmDiscard = $state<WifiBand | undefined | null>(null);
  let initialized = $state(false);

  const network = $derived(status.ap.networks.find((item) => item.band === selectedBand) ?? status.ap.networks[0]);
  const hasChanges = $derived(Boolean(network) && (ssid !== network.ssid || password.length > 0));

  function bandLabel(band: WifiBand | undefined): string {
    return band === "2g" ? "2,4 ГГц" : band === "5g" ? "5 ГГц" : "Wi-Fi";
  }

  $effect(() => {
    if (!initialized && network) {
      selectedBand = network.band;
      ssid = network.ssid;
      initialized = true;
    }
  });

  function loadBand(band: WifiBand | undefined) {
    const next = status.ap.networks.find((item) => item.band === band);
    if (!next) return;
    selectedBand = band;
    ssid = next.ssid;
    password = "";
    error = "";
  }

  function chooseBand(band: WifiBand | undefined) {
    if (band === selectedBand) return;
    if (hasChanges) {
      confirmDiscard = band;
      return;
    }
    loadBand(band);
  }

  function reset() {
    if (!network) return;
    ssid = network.ssid;
    password = "";
    error = "";
  }

  function discardAndSwitch() {
    const band = confirmDiscard;
    if (band === null) return;
    loadBand(band);
    confirmDiscard = null;
  }

  function requestSave(event: SubmitEvent) {
    event.preventDefault();
    const nextSsid = ssid.trim();
    if (!nextSsid || new TextEncoder().encode(nextSsid).length > 32 || /[\x00-\x1f\x7f]/.test(nextSsid)) {
      error = "SSID: от 1 до 32 байт без управляющих символов.";
      return;
    }
    if (password && !/^[\x20-\x7e]{8,63}$/.test(password)) {
      error = "Пароль Wi-Fi: 8-63 печатных ASCII-символа.";
      return;
    }
    error = "";
    confirmSave = true;
  }

  async function save() {
    if (!network) return;
    confirmSave = false;
    const nextSsid = ssid.trim();
    if (await app.saveAp(selectedBand, nextSsid, password)) {
      ssid = nextSsid;
      password = "";
    }
  }
</script>

<svelte:head><title>Сеть · Gofro Router</title></svelte:head>

{#if reconnectSsid}
  <section class="panel p-6" role="status" aria-live="assertive">
    <h2>Подключитесь заново</h2>
    <p class="notice">Точка доступа перезапускается. Выберите сеть <strong>{reconnectSsid}</strong>, затем вернитесь на <a href="https://wifi.gofro.net">https://wifi.gofro.net</a>.</p>
    <button class="btn primary" type="button" onclick={app.resumePolling}>Я подключился</button>
  </section>
{:else if network}
  <section class="panel" aria-labelledby="wifi-title">
    <div class="panel-head"><h2 id="wifi-title">Домашний Wi-Fi</h2></div>
    <div class="panel-body">
      <div class="segmented" aria-label="Диапазон Wi-Fi">
        {#each status.ap.networks as item (item.band)}
          <button type="button" aria-pressed={item.band === selectedBand} disabled={busy} onclick={() => chooseBand(item.band)}>{bandLabel(item.band)}</button>
        {/each}
      </div>
      <form class="wifi-form" onsubmit={requestSave}>
        <label class="field">Имя сети<input bind:value={ssid} required maxlength="32" autocomplete="off" disabled={busy} /></label>
        <div>
          <PasswordInput label="Новый пароль" bind:value={password} minlength={8} maxlength={63} autocomplete="new-password" placeholder="Оставить текущий" disabled={busy} />
          <span class="field-help">8-63 символа: латинские буквы, цифры или знаки.</span>
        </div>
        {#if error || app.actionError}<p class="error" role="alert">{error || app.actionError}</p>{/if}
        <div class="form-actions"><button class="btn ghost" type="button" onclick={reset} disabled={!hasChanges || busy}>Отменить</button><button class="btn primary" type="submit" disabled={busy || !hasChanges}>{busy ? "Сохраняем…" : "Сохранить"}</button></div>
      </form>
    </div>
  </section>
{:else}
  <section class="panel"><div class="empty"><h2>Нет доступных сетей Wi-Fi</h2><p>Обновите состояние роутера и попробуйте снова.</p></div></section>
{/if}

{#if confirmSave}<SharedDialog title="Сохранить настройки Wi-Fi?" onclose={() => confirmSave = false} busy={busy}><p class="notice">Точка доступа перезапустится. Устройство нужно будет подключить к сети заново.</p><div class="form-actions"><button class="btn ghost" type="button" onclick={() => confirmSave = false} disabled={busy}>Отмена</button><button class="btn primary" type="button" onclick={save} disabled={busy}>Сохранить</button></div></SharedDialog>{/if}
{#if confirmDiscard !== null}<SharedDialog title="Отменить изменения?" onclose={() => confirmDiscard = null}><p class="notice">Несохраненные изменения для этого диапазона будут потеряны.</p><div class="form-actions"><button class="btn ghost" type="button" onclick={() => confirmDiscard = null}>Остаться</button><button class="btn primary" type="button" onclick={discardAndSwitch}>Переключить</button></div></SharedDialog>{/if}
