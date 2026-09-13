<script lang="ts">
  import { onMount } from "svelte";
  import { getAppContext } from "../app-context";
  import PasswordInput from "../components/password-input.svelte";

  const app = getAppContext();
  const authState = $derived(app.authState);
  const authError = $derived(app.authError);
  const setupMethod = $derived(app.setupMethod);
  const setupWindowSeconds = $derived(app.setupWindowSeconds);
  let now = $state(Date.now());
  let deadline = $state<number | null>(null);
  let setupCode = $state("");
  let password = $state("");
  let confirmation = $state("");
  let validationError = $state("");
  let submitting = $state(false);

  $effect(() => {
    const receivedAt = Date.now();
    now = receivedAt;
    deadline = setupMethod === "code" && setupWindowSeconds !== null
      ? receivedAt + setupWindowSeconds * 1_000
      : null;
  });

  onMount(() => {
    const interval = window.setInterval(() => now = Date.now(), 1_000);
    return () => window.clearInterval(interval);
  });

  const setupSecondsLeft = $derived(deadline === null ? null : Math.max(0, Math.ceil((deadline - now) / 1_000)));
  const setupExpired = $derived(app.setupClosed || (authState === "setup" && setupMethod === "code" && (setupWindowSeconds === 0 || setupSecondsLeft === 0)));

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (submitting) return;
    if (!password || (authState === "setup" && (!setupCode || password !== confirmation))) {
      validationError = authState === "setup"
        ? "Введите код установки и два одинаковых пароля администратора."
        : "Введите пароль администратора.";
      return;
    }
    if (authState === "setup" && Array.from(password).length < 8) {
      validationError = "Пароль администратора должен содержать не менее 8 символов.";
      return;
    }
    if (new TextEncoder().encode(password).length > 128) {
      validationError = "Пароль слишком длинный. Максимум 128 байт: кириллица занимает больше одного байта на символ.";
      return;
    }
    validationError = "";
    submitting = true;
    try {
      const success = authState === "setup"
        ? await app.setupAuth(password, setupCode)
        : await app.loginAuth(password);
      if (success) {
        setupCode = "";
        password = "";
        confirmation = "";
      }
    } finally {
      submitting = false;
    }
  }
</script>

<svelte:head><title>Вход · Gofro VPN</title></svelte:head>

<main class="access-page">
  <section class="access-card" aria-labelledby="access-title">
    <div class="brand access-brand"><strong>Gofro</strong><span>VPN</span></div>
    {#if authState === "setup" && setupMethod === "code"}
      <ol class="setup-progress" aria-label="Ход настройки"><li class="current"><span>1</span>Пароль</li><li><span>2</span>VPN</li></ol>
    {/if}
    <div class="access-copy">
      <h1 id="access-title">{authState === "setup" ? "Придумайте пароль" : "Вход в панель"}</h1>
      {#if authState === "setup" && setupMethod === "code" && setupSecondsLeft !== null}<p class="access-note">Введите одноразовый код из консоли <code>gofro setup</code>. Код действует 15 минут. Осталось: {Math.floor(setupSecondsLeft / 60)}:{String(setupSecondsLeft % 60).padStart(2, "0")}</p>{/if}
      {#if authState === "setup" && setupMethod !== "code"}
        <p class="error" role="alert">Требуется миграция OpenWrt. Обновите Gofro VPN: настройка доступна только по одноразовому коду `gofro setup`.</p>
      {:else}<form class:mt-0={authState === "login"} onsubmit={submit}>
        {#if authState === "setup"}
          <PasswordInput label="Код установки" bind:value={setupCode} required autocomplete="one-time-code" disabled={submitting || setupExpired} />
        {/if}
        <div>
          <PasswordInput label="Пароль" bind:value={password} required minlength={authState === "setup" ? 8 : undefined} maxlength={128} autocomplete={authState === "setup" ? "new-password" : "current-password"} aria-describedby={authState === "setup" ? "password-requirement" : undefined} disabled={submitting || setupExpired} />
          {#if authState === "setup"}<span class="field-help" id="password-requirement">Минимум 8 символов, максимум 128 байт.</span>{/if}
        </div>
        {#if authState === "setup"}<PasswordInput label="Повторите пароль" bind:value={confirmation} required minlength={8} maxlength={128} autocomplete="new-password" disabled={submitting || setupExpired} />{/if}
        {#if validationError || authError}<p class="error" role="alert">{validationError || authError}</p>{/if}
        {#if setupExpired}<p class="error" role="alert">Время настройки истекло; запустите команду установки в консоли роутера повторно.</p>{/if}
        <button class="btn primary access-submit" type="submit" disabled={submitting || setupExpired}>{submitting ? "Проверяем…" : authState === "setup" ? "Продолжить" : "Войти"}</button>
      </form>{/if}
    </div>
    {#if authError || setupExpired}<button class="btn ghost access-submit" type="button" disabled={submitting} onclick={app.initializeAuth}>Повторить проверку</button>{/if}
  </section>
</main>
