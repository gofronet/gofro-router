<script lang="ts">
  import { getAppContext } from "../app-context";
  import PasswordInput from "../components/password-input.svelte";

  const app = getAppContext();
  const authState = $derived(app.authState);
  const authError = $derived(app.authError);
  let setupCode = $state("");
  let password = $state("");
  let confirmation = $state("");
  let validationError = $state("");
  let submitting = $state(false);

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (submitting) return;
    if (!password || (authState === "setup" && (!setupCode || password !== confirmation))) {
      validationError = authState === "setup"
        ? "Введите пароль Wi-Fi и два одинаковых пароля администратора."
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
        ? await app.setupAuth(setupCode, password)
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

<main class="grid min-h-dvh gap-8 bg-[#f5f5f5] p-4 text-[#09090b] sm:p-6 lg:grid-cols-2">
  <section class="hidden min-w-0 flex-col justify-between gap-12 rounded-[28px] bg-[linear-gradient(145deg,#202024,#09090b_72%)] p-8 text-white lg:flex">
    <span class="flex size-14 items-end justify-center gap-1 rounded-2xl bg-white px-3 py-3"><i class="h-2 w-1 rounded-sm bg-[#09090b]"></i><i class="h-4 w-1 rounded-sm bg-[#09090b]"></i><i class="h-7 w-1 rounded-sm bg-[#09090b]"></i></span>
    <div class="max-w-md"><span class="text-xs font-bold tracking-[0.18em] text-[#aaaab1] uppercase">Gofro Router</span><h2 class="mt-3 text-[clamp(2rem,4vw,3rem)] leading-tight font-extrabold tracking-[-0.07em]">Ваша сеть. Под вашим контролем.</h2><p class="mt-4 text-sm leading-relaxed text-[#aaaab1]">Управление доступно только после защищенного входа.</p></div>
  </section>
  <section class="mx-auto flex w-full min-w-0 max-w-md flex-col justify-center py-3 lg:px-4">
    <span class="mb-5 flex size-10 shrink-0 items-end justify-center gap-1 rounded-xl bg-[#09090b] p-2.5 lg:hidden"><i class="h-1.5 w-1 rounded-sm bg-white"></i><i class="h-3 w-1 rounded-sm bg-white"></i><i class="h-5 w-1 rounded-sm bg-white"></i></span>
    <span class="text-xs font-bold tracking-[0.18em] text-[#74747d] uppercase">Безопасный доступ</span>
    <h1 class="mt-2 text-3xl leading-tight font-extrabold tracking-[-0.06em] sm:text-4xl">{authState === "setup" ? "Создайте пароль" : "Войдите"}</h1>
    <p class="mt-3 text-sm leading-relaxed text-[#74747d]">{authState === "setup" ? "Подтвердите текущий пароль Wi-Fi роутера и задайте отдельный пароль администратора." : "Введите пароль администратора для управления роутером."}</p>
    <form class="mt-5 grid gap-3" onsubmit={submit}>
      {#if authState === "setup"}
        <PasswordInput label="Пароль Wi-Fi" bind:value={setupCode} required autocomplete="off" disabled={submitting} />
      {/if}
      <div>
        <PasswordInput label={authState === "setup" ? "Новый пароль администратора" : "Пароль администратора"} bind:value={password} required minlength={authState === "setup" ? 12 : undefined} maxlength={128} autocomplete={authState === "setup" ? "new-password" : "current-password"} aria-describedby={authState === "setup" ? "password-requirement" : undefined} disabled={submitting} />
        {#if authState === "setup"}<p class="mt-1.5 text-xs text-[#74747d]" id="password-requirement">Не менее 12 символов.</p>{/if}
      </div>
      {#if authState === "setup"}<PasswordInput label="Повторите пароль" bind:value={confirmation} required minlength={12} maxlength={128} autocomplete="new-password" disabled={submitting} />{/if}
      {#if validationError || authError}<p class="m-0 rounded-2xl border border-red-200 bg-red-50 p-3 text-xs leading-relaxed text-red-700" role="alert">{validationError || authError}</p>{/if}
      <button class="mt-1 min-h-12 rounded-2xl border border-[#09090b] bg-[#09090b] px-5 text-sm font-bold text-white disabled:cursor-wait disabled:opacity-60" type="submit" disabled={submitting}>{submitting ? "Проверяем…" : authState === "setup" ? "Создать пароль" : "Войти"}</button>
    </form>
    {#if authError}<button class="mt-2 min-h-11 text-sm font-bold text-[#74747d]" type="button" disabled={submitting} onclick={app.initializeAuth}>Повторить проверку</button>{/if}
    <details class="mt-4 text-xs leading-relaxed text-[#74747d]"><summary class="cursor-pointer py-2">О предупреждении HTTPS</summary><p class="mt-1">У роутера собственный сертификат. Подтверждайте исключение только для вашего устройства. Пароли передаются по HTTPS.</p></details>
  </section>
</main>
