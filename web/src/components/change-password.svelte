<script lang="ts">
  import { getAppContext } from "../app-context";
  import Dialog from "./dialog.svelte";
  import PasswordInput from "./password-input.svelte";

  const app = getAppContext();
  const id = $props.id();
  let open = $state(false);
  let currentPassword = $state("");
  let password = $state("");
  let confirmation = $state("");
  let error = $state("");
  let success = $state(false);
  let submitting = $state(false);

  function clearSecrets() {
    currentPassword = password = confirmation = "";
  }

  function close() {
    if (submitting) return;
    clearSecrets();
    error = "";
    open = false;
  }

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (submitting || app.busy) return;
    error = "";
    if (!currentPassword) error = "Введите текущий пароль.";
    else if (Array.from(password).length < 8) error = "Пароль администратора должен содержать не менее 8 символов.";
    else if (new TextEncoder().encode(password).length > 128) error = "Пароль слишком длинный. Максимум 128 байт: кириллица занимает больше одного байта на символ.";
    else if (password !== confirmation) error = "Новые пароли не совпадают.";
    if (error) return;
    submitting = true;
    try {
      success = await app.changePassword(currentPassword, password);
      if (success) open = false;
    } catch (cause) {
      error = cause instanceof Error ? cause.message : "Не удалось сменить пароль.";
    } finally {
      clearSecrets();
      submitting = false;
    }
  }
</script>

<section class="panel">
  <div class="full-row"><div class="row-main"><h3>Сменить пароль</h3></div><button class="btn" type="button" disabled={app.busy} onclick={() => { success = false; open = true; }}>Сменить пароль</button></div>
  {#if success}<p class="panel-body" role="status">Пароль изменён. Остальные сеансы завершены.</p>{/if}
</section>
{#if open}
  <Dialog title="Сменить пароль" onclose={close} busy={submitting}>
    <form onsubmit={submit} aria-busy={submitting}>
      <PasswordInput label="Текущий пароль" bind:value={currentPassword} autocomplete="current-password" required disabled={submitting} />
      <PasswordInput label="Новый пароль" bind:value={password} autocomplete="new-password" aria-describedby={`${id}-requirements`} required disabled={submitting} />
      <p class="field-help" id={`${id}-requirements`}>Минимум 8 символов, максимум 128 байт. Остальные сеансы будут завершены.</p>
      <PasswordInput label="Повторите новый пароль" bind:value={confirmation} autocomplete="new-password" required disabled={submitting} />
      {#if error}<p class="error" role="alert">{error}</p>{/if}
      <div class="form-actions"><button class="btn ghost" type="button" disabled={submitting} onclick={close}>Отмена</button><button class="btn primary" type="submit" disabled={submitting || app.busy}>{submitting ? "Сохраняем…" : "Сменить пароль"}</button></div>
    </form>
  </Dialog>
{/if}
