<script lang="ts">
  import Check from "lucide-svelte/icons/check";
  import LoaderCircle from "lucide-svelte/icons/loader-circle";
  import CircleAlert from "lucide-svelte/icons/circle-alert";
  import PasswordInput from "./password-input.svelte";
  import { vpsHostInputSchema } from "../api/schemas";
  import type { BootstrapStage } from "../domain/models";
  import type { RouterState } from "../stores/router-state.svelte";

  let { app, running = $bindable(false), onadded, onclose }: {
    app: Pick<RouterState, "busy" | "actionError" | "bootstrapServer" | "resetHostPin">;
    running?: boolean;
    onadded?: () => Promise<void>;
    onclose: () => void;
  } = $props();

  let name = $state("");
  let host = $state("");
  let port = $state("22");
  let password = $state("");
  let result = $state<"idle" | "failed" | "complete">("idle");
  const setup = $derived(running ? "running" : result);
  const busy = $derived(app.busy || running);
  let stages = $state<BootstrapStage[]>([]);
  let error = $state("");
  let attemptedTarget = $state<{ host: string; port: number } | null>(null);
  const hostKeyChanged = $derived(setup === "failed" && (error || app.actionError).includes("SSH host key changed"));
  const stageLabels: Record<BootstrapStage, string> = {
    waiting: "Ожидаем начало настройки",
    host_key: "Проверяем ключ SSH",
    connect: "Подключаемся к VPS",
    inspect: "Проверяем сервер",
    install: "Устанавливаем Gofro VPN",
    authorize: "Настраиваем доступ роутера",
    profile: "Создаём профиль VPN",
    save: "Сохраняем сервер на роутере",
  };

  async function bootstrap(event: SubmitEvent) {
    event.preventDefault();
    if (busy) return;
    const sshPort = Number(port);
    if (!name.trim() || !host.trim() || !password || !Number.isInteger(sshPort) || sshPort < 1 || sshPort > 65535) {
      error = "Введите название, публичный IPv4-адрес, SSH-порт и пароль root.";
      return;
    }
    if (Array.from(name.trim()).length > 60) {
      error = "Название не должно быть длиннее 60 символов.";
      return;
    }
    const target = vpsHostInputSchema.safeParse(host.trim());
    if (!target.success) {
      error = target.error.issues[0].message;
      return;
    }
    error = "";
    attemptedTarget = { host: target.data, port: sshPort };
    running = true;
    stages = ["waiting"];
    try {
      const completed = await app.bootstrapServer(name.trim(), target.data, sshPort, password, (stage) => {
        if (!stages.includes(stage)) stages = [...stages, stage];
      });
      result = completed ? "complete" : "failed";
      running = false;
      if (!completed) error = app.actionError || "Не удалось подтвердить результат настройки. Проверьте список серверов и состояние VPS перед ручной повторной попыткой.";
    } finally {
      password = "";
    }
    if (setup === "complete") await onadded?.();
  }

  async function resetHostPin() {
    const target = attemptedTarget;
    if (!hostKeyChanged || !target || !confirm(`Сбросить сохранённый ключ SSH для ${target.host}:${target.port}? Продолжайте только после проверки VPS через доверенную консоль.`)) return;
    error = "";
    if (await app.resetHostPin(target.host, target.port)) {
      error = "Сохранённый ключ SSH сброшен. Введите пароль root и повторите настройку.";
    }
  }
</script>

{#if setup !== "idle"}
  <section class="bootstrap-progress" aria-label="Настройка VPS">
    <div class="bootstrap-heading">
      <span class="bootstrap-symbol" aria-hidden="true">
        {#if setup === "running"}
          <LoaderCircle class="motion-safe:animate-spin" size={25} />
        {:else if setup === "complete"}
          <Check size={25} />
        {:else}
          <CircleAlert size={25} />
        {/if}
      </span>
      <div>
        <h3>{setup === "running" ? "Настраиваем ваш сервер" : setup === "complete" ? "Сервер добавлен" : "Настройка требует внимания"}</h3>
        <p class="mono">{host}:{port}</p>
      </div>
    </div>
    <p class="bootstrap-caption">
      {#if setup === "running"}
        Роутер подключается к VPS и выполняет настройку. Не закрывайте эту страницу.
      {:else if setup === "complete"}
        Профиль сохранён на роутере. Состояние VPN можно проверить на главной странице.
      {:else}
        Изменения могли сохраниться. Перед ручной повторной попыткой проверьте список серверов, состояние VPN на роутере и состояние VPS. Затем введите пароль заново.
      {/if}
    </p>
    <p class="sr-only" role="status" aria-live="polite" aria-atomic="true">
      {setup === "complete" ? "Сервер добавлен" : setup === "failed" ? "Настройка требует внимания" : stageLabels[stages[stages.length - 1] ?? "waiting"]}
    </p>
    <ol class="bootstrap-stages">
      {#each stages as stage, index (stage)}
        {@const done = index < stages.length - 1 || setup === "complete"}
        <li class:stage-done={done} class:stage-current={!done} aria-current={!done ? "step" : undefined}>
          <span class="stage-symbol" aria-hidden="true">
            {#if done}
              <Check size={15} />
            {:else if setup === "running"}
              <LoaderCircle class="motion-safe:animate-spin" size={16} />
            {:else}
              <CircleAlert size={16} />
            {/if}
          </span>
          <span>
            {stageLabels[stage]}<span class="sr-only">: {done ? "завершено" : setup === "failed" ? "результат не подтверждён" : "выполняется"}</span>
          </span>
        </li>
      {/each}
    </ol>
  </section>
{/if}
{#if setup === "complete"}
  <div class="form-actions">
    <button class="btn primary" type="button" disabled={busy} onclick={onclose}>Готово</button>
  </div>
{:else if setup !== "running"}
  {#if error || app.actionError}
    <p class="error bootstrap-error" role="alert">{error || app.actionError}</p>
  {/if}
  {#if hostKeyChanged}
    <div class="form-actions bootstrap-reset">
      <button class="btn" type="button" disabled={busy} onclick={resetHostPin}>Сбросить сохранённый ключ SSH</button>
    </div>
  {/if}
  <form onsubmit={bootstrap}>
    <label class="field">
      Название
      <input bind:value={name} required maxlength="120" autocomplete="off" />
    </label>
    <label class="field">
      Публичный IPv4-адрес VPS
      <input bind:value={host} required maxlength="255" inputmode="text" autocomplete="off" />
      <span class="field-help">IPv6 не поддерживается. IPv4-адрес можно получить в панели провайдера VPS.</span>
    </label>
    <div class="form-line">
      <label class="field">
        Логин
        <input value="root" readonly />
      </label>
      <label class="field">
        Порт SSH
        <input bind:value={port} type="number" min="1" max="65535" />
      </label>
    </div>
    <PasswordInput label="Пароль root" bind:value={password} required autocomplete="off" disabled={busy} />
    <p class="field-help">Пароль используется только для настройки и не сохраняется.</p>
    <div class="form-actions">
      <button class="btn primary" type="submit" disabled={busy}>
        {setup === "failed" ? "Повторить настройку" : "Настроить сервер"}
      </button>
    </div>
  </form>
{/if}

<style>
  .bootstrap-progress { border: 1px solid var(--border); border-radius: 10px; background: var(--bg); padding: 20px; margin-bottom: 20px; }
  .bootstrap-heading { display: flex; align-items: center; gap: 13px; }
  .bootstrap-heading > div { min-width: 0; }
  .bootstrap-heading h3 { font-size: 15px; }
  .bootstrap-heading p { margin-top: 3px; color: var(--muted); }
  .bootstrap-symbol { display: grid; flex: 0 0 auto; place-items: center; width: 46px; height: 46px; border: 1px solid var(--border); border-radius: 50%; background: var(--surface); color: var(--accent); }
  .bootstrap-caption { margin-top: 16px; color: var(--muted); font-size: 12px; }
  .bootstrap-stages { display: grid; gap: 0; list-style: none; margin: 18px 0 0; padding: 0; }
  .bootstrap-stages li { display: flex; align-items: center; gap: 10px; min-height: 34px; font-size: 12px; }
  .stage-symbol { display: grid; flex: 0 0 auto; place-items: center; width: 24px; height: 24px; }
  .stage-done { color: var(--muted); }
  .stage-done .stage-symbol { color: var(--accent); }
  .stage-current { color: var(--fg); font-weight: 600; }
  .bootstrap-error { margin-bottom: 20px; white-space: pre-wrap; overflow-wrap: anywhere; }
  .bootstrap-reset { justify-content: flex-start; margin-bottom: 20px; }
</style>
