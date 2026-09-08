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
  let firstLaunchHelp = $state(false);

  $effect(() => {
    deadline = setupMethod === "local" && setupWindowSeconds !== null
      ? Date.now() + setupWindowSeconds * 1_000
      : null;
  });

  onMount(() => {
    const interval = window.setInterval(() => now = Date.now(), 1_000);
    return () => window.clearInterval(interval);
  });

  const setupSecondsLeft = $derived(deadline === null ? null : Math.max(0, Math.ceil((deadline - now) / 1_000)));
  const setupExpired = $derived(app.setupClosed || (authState === "setup" && setupMethod === "local" && (setupWindowSeconds === 0 || setupSecondsLeft === 0)));

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (submitting) return;
    if (!password || (authState === "setup" && ((setupMethod === "wifi_password" && !setupCode) || password !== confirmation))) {
      validationError = authState === "setup"
        ? setupMethod === "wifi_password" ? "Введите пароль Wi-Fi и два одинаковых пароля администратора." : "Введите два одинаковых пароля администратора."
        : "Введите пароль администратора.";
      return;
    }
    if (authState === "setup" && Array.from(password).length < 12) {
      validationError = "Пароль администратора должен содержать не менее 12 символов.";
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
        ? await app.setupAuth(password, setupMethod === "wifi_password" ? setupCode : undefined)
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

<svelte:head><title>Вход · Gofro Router</title></svelte:head>

<main class="access-page">
  <section class="access-card" aria-labelledby="access-title">
    <div class="brand access-brand"><strong>Gofro</strong><span>Router</span></div>
    {#if authState === "setup" && setupMethod === "local"}
      <ol class="setup-progress" aria-label="Ход настройки"><li class="current"><span>1</span>Пароль</li><li><span>2</span>Wi-Fi</li><li><span>3</span>VPN</li></ol>
    {/if}
    <div class="access-copy">
      <h1 id="access-title">{authState === "setup" ? "Придумайте пароль" : "Вход в панель"}</h1>
      {#if authState === "setup" && setupMethod === "local" && setupSecondsLeft !== null}<p class="access-note">Окно настройки: {Math.floor(setupSecondsLeft / 60)}:{String(setupSecondsLeft % 60).padStart(2, "0")}</p>{/if}
      <form class:mt-0={authState === "login"} onsubmit={submit}>
        {#if authState === "setup" && setupMethod === "wifi_password"}
          <PasswordInput label="Пароль Wi-Fi" bind:value={setupCode} required autocomplete="off" disabled={submitting || setupExpired} />
        {/if}
        <div>
          <PasswordInput label="Пароль" bind:value={password} required minlength={authState === "setup" ? 12 : undefined} maxlength={128} autocomplete={authState === "setup" ? "new-password" : "current-password"} aria-describedby={authState === "setup" ? "password-requirement" : undefined} disabled={submitting || setupExpired} />
          {#if authState === "setup"}<span class="field-help" id="password-requirement">Минимум 12 символов, максимум 128 байт.</span>{/if}
        </div>
        {#if authState === "setup"}<PasswordInput label="Повторите пароль" bind:value={confirmation} required minlength={12} maxlength={128} autocomplete="new-password" disabled={submitting || setupExpired} />{/if}
        {#if validationError || authError}<p class="error" role="alert">{validationError || authError}</p>{/if}
        {#if setupExpired}<p class="error" role="alert">Время настройки истекло; запустите команду установки в консоли роутера повторно.</p>{/if}
        <button class="btn primary access-submit" type="submit" disabled={submitting || setupExpired}>{submitting ? "Проверяем…" : authState === "setup" ? "Продолжить" : "Войти"}</button>
      </form>
    </div>
    {#if authState === "login"}
      <button class="access-link" type="button" aria-expanded={firstLaunchHelp} onclick={() => firstLaunchHelp = !firstLaunchHelp}>Первый запуск</button>
      {#if firstLaunchHelp}<p class="access-note" role="status">Для первой настройки подключитесь к Wi-Fi роутера и откройте панель. Мастер станет доступен только в новом сеансе настройки.</p>{/if}
    {/if}
    {#if authError}<button class="btn ghost access-submit" type="button" disabled={submitting} onclick={app.initializeAuth}>Повторить проверку</button>{/if}
  </section>
</main>
