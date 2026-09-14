import { api, ApiError } from "../api";
import { clearCsrfToken, setCsrfToken } from "../api/client";
import { deviceExclusionInputSchema, deviceExclusionsSchema } from "../api/schemas";
import type {
  ProfileInput,
  AuthStatus,
  OnboardingStatus,
  RoutingConfig,
  RoutingTest,
  ServerInput,
  BootstrapStage,
  ServerVersion,
  ManagedServerStatus,
  Profile,
  Status,
  DeviceExclusionInput,
  LanDevices,
} from "../domain/models";

const refreshRequired = "Состояние после изменения не подтверждено. Сначала обновите состояние; новая команда не отправлена.";

export class RouterState {
  private currentStatus = $state<Status | null>(null);
  private pollInFlight = false;
  private statusVersion = 0;
  private interval: number | null = null;
  private onboardingInFlight = false;
  private onboardingVersion = 0;
  private generation = 0;
  private authVersion = 0;

  loading = $state(true);
  authLoading = $state(true);
  authState = $state<AuthStatus["state"]>("login");
  authError = $state("");
  setupClosed = $state(false);
  onboardingLoading = $state(false);
  onboarding = $state<OnboardingStatus | null>(null);
  pollError = $state("");
  actionError = $state("");
  actionWarning = $state("");
  statusUncertain = $state(false);
  private warningKind = $state("");
  mutation = $state<string | null>(null);
  lanDevices = $state<LanDevices>({ devices: [], discovery: "unavailable" });
  inventoryLoading = $state(false);
  inventoryError = $state("");

  get warningServerKey(): string | null {
    return /^(?:update|restart|friend-create|friend-rename|friend-revoke):(.+)$/.exec(this.warningKind)?.[1] ?? null;
  }

  get status(): Status {
    if (!this.currentStatus) throw new Error("Status is not loaded yet");
    return this.currentStatus;
  }

  get hasStatus(): boolean {
    return this.currentStatus !== null;
  }

  get busy(): boolean {
    return this.mutation !== null;
  }

  get connected(): boolean {
    if (this.statusUncertain) return false;
    const status = this.currentStatus;
    return Boolean(status?.vpn_enabled && status.tunnel_active && status.peer?.handshake_age_seconds != null && status.peer.handshake_age_seconds <= 180);
  }

  private message(error: unknown): string {
    if (!(error instanceof Error)) return "Неизвестная ошибка";
    switch (error.message) {
      case "password_too_short": return "Пароль администратора должен содержать не менее 8 символов.";
      case "password_too_long": return "Пароль слишком длинный. Максимум 128 байт: кириллица занимает больше одного байта на символ.";
      case "invalid_setup_code": return "Неверный одноразовый код установки.";
      case "invalid_password": return "Неверный пароль администратора.";
      case "invalid_current_password": return "Неверный текущий пароль.";
      case "request_rejected": return "Проверка безопасности не пройдена. Обновите страницу и попробуйте снова.";
      case "auth_busy": return "Уже выполняется проверка пароля. Подождите и попробуйте снова.";
      case "setup_completed": return "Пароль уже создан. Обновите страницу и войдите.";
      case "setup_closed": return "Время настройки истекло; запустите команду установки в консоли роутера повторно.";
      case "onboarding_required": return "Сначала завершите мастер настройки.";
      case "setup_required": return "Сначала создайте пароль администратора. Обновите страницу.";
      case "login_throttled": return "Слишком частые попытки входа. Подождите секунду.";
      case "session_expired": return "Сессия истекла. Войдите снова.";
      case "internal_error": return "Не удалось выполнить вход. Попробуйте ещё раз.";
      default: return error.message;
    }
  }

  private applyAuth = (auth: AuthStatus): void => {
    this.onboardingVersion++;
    this.authState = auth.state;
    this.auth = auth;
    this.setupClosed = false;
    setCsrfToken(auth.csrf_token);
  };

  private auth = $state<AuthStatus | null>(null);

  get setupMethod(): "code" | "local" | "wifi_password" | null {
    return this.auth?.state === "setup" ? this.auth.setup_method : null;
  }

  get setupWindowSeconds(): number | null {
    return this.auth?.state === "setup" && (this.auth.setup_method === "code" || this.auth.setup_method === "local")
      ? this.auth.setup_window_seconds
      : null;
  }

  private handleAuthError = (error: unknown): boolean => {
    if (!(error instanceof ApiError) || error.status !== 401) return false;
    this.stop();
    this.onboardingVersion++;
    this.statusVersion++;
    this.currentStatus = null;
    this.loading = false;
    this.authState = "login";
    this.auth = null;
    this.stopPolling();
    this.onboarding = null;
    return true;
  };

  private mutateResult = async <T>(
    kind: string,
    operation: () => Promise<T>,
    onCommitted?: () => void,
  ): Promise<T | null> => {
    if (this.busy) return null;

    this.statusVersion++;
    const generation = this.generation;
    this.mutation = kind;
    this.actionError = "";

    try {
      const result = await operation();
      if (generation !== this.generation) return null;
      this.pollError = "";
      return result;
    } catch (error) {
      if (generation !== this.generation || this.handleAuthError(error)) return null;
      if (error instanceof ApiError && error.outcome === "committed") {
        this.warningKind = kind;
        this.actionWarning = "Изменение выполнено, но состояние не удалось обновить. Обновите состояние отдельно; повторять изменение не нужно.";
        onCommitted?.();
        return null;
      }
      this.actionError = this.message(error);
      return null;
    } finally {
      if (generation === this.generation) this.mutation = null;
    }
  };

  private mutate = async (
    kind: string,
    operation: () => Promise<Status>,
  ): Promise<boolean> => {
    if (this.busy) return false;
    const generation = this.generation;
    let committed = false;
    const result = await this.mutateResult(kind, operation, () => { committed = true; });
    if (generation !== this.generation) return false;
    if (!result) {
      this.statusUncertain = true;
      return committed;
    }
    this.currentStatus = result;
    this.statusUncertain = false;
    if (!this.warningServerKey) this.actionWarning = "";
    return true;
  };

  refresh = async (): Promise<void> => {
    if (this.pollInFlight || this.mutation) return;

    this.pollInFlight = true;
    const version = this.statusVersion;
    const authVersion = this.authVersion;
    const generation = this.generation;

    try {
      const nextStatus = await api.status.get();
      if (generation === this.generation && version === this.statusVersion) {
        this.currentStatus = nextStatus;
        this.statusUncertain = false;
        if (this.actionError === refreshRequired) this.actionError = "";
        this.pollError = "";
        if (!this.warningServerKey) this.actionWarning = "";
      }
    } catch (error) {
      if (generation !== this.generation || authVersion !== this.authVersion) return;
      if (this.handleAuthError(error)) return;
      if (version === this.statusVersion) {
        this.pollError = this.message(error);
      }
    } finally {
      if (generation === this.generation) {
        this.loading = false;
        this.pollInFlight = false;
      }
    }
  };

  startPolling(): void {
    if (this.interval !== null) return;
    const generation = this.generation;
    void this.refresh();
    this.interval = window.setInterval(() => {
      if (generation === this.generation) void this.refresh();
    }, 5_000);
  }

  stopPolling(): void {
    if (this.interval === null) return;
    window.clearInterval(this.interval);
    this.interval = null;
  }

  stop(): void {
    this.generation++;
    this.authVersion++;
    this.statusUncertain = true;
    this.onboardingVersion++;
    this.stopPolling();
    this.pollInFlight = false;
    this.onboardingInFlight = false;
    this.onboardingLoading = false;
    this.authLoading = false;
    this.loading = false;
    this.mutation = null;
    this.actionWarning = "";
    this.lanDevices = { devices: [], discovery: "unavailable" };
    this.inventoryLoading = false;
    this.inventoryError = "";
  }

  loadOnboarding = async (): Promise<void> => {
    if (this.authState !== "authenticated" || this.onboardingInFlight) return;
    const version = this.onboardingVersion;
    const generation = this.generation;
    const authVersion = this.authVersion;
    this.onboardingInFlight = true;
    this.onboardingLoading = true;
    try {
      const onboarding = await api.onboarding.get();
      if (generation !== this.generation || version !== this.onboardingVersion || this.authState !== "authenticated") return;
      this.onboarding = onboarding;
      if (onboarding.step !== "complete") this.stopPolling();
      if (onboarding.step === "server" && (!this.hasStatus || this.statusUncertain)) await this.refresh();
      if (generation !== this.generation) return;
      if (onboarding.step === "complete") this.startPolling();
    } catch (error) {
      if (generation !== this.generation || authVersion !== this.authVersion || version !== this.onboardingVersion || this.authState !== "authenticated" || this.handleAuthError(error)) return;
      this.actionError = this.message(error);
    } finally {
      if (generation === this.generation && version === this.onboardingVersion) {
        this.onboardingLoading = false;
        this.onboardingInFlight = false;
      }
    }
  };

  clearActionError = (): void => {
    this.actionError = "";
  };

  initializeAuth = async (): Promise<void> => {
    this.stop();
    const authVersion = this.authVersion;
    const generation = this.generation;
    this.authLoading = true;
    this.authError = "";
    try {
      const auth = await api.auth.status();
      if (generation !== this.generation || authVersion !== this.authVersion) return;
      this.applyAuth(auth);
      this.loading = auth.state === "authenticated";
      if (auth.state === "authenticated") await this.loadOnboarding();
    } catch (error) {
      if (generation !== this.generation || authVersion !== this.authVersion) return;
      clearCsrfToken();
      this.authState = "login";
      this.auth = null;
      this.authError = this.message(error);
      this.loading = false;
    } finally {
      if (generation === this.generation && authVersion === this.authVersion) this.authLoading = false;
    }
  };

  setupAuth = async (password: string, setupCode?: string): Promise<boolean> => {
    this.stop();
    const authVersion = this.authVersion;
    const generation = this.generation;
    this.authError = "";
    try {
      const auth = await api.auth.setup(password, setupCode);
      if (generation !== this.generation || authVersion !== this.authVersion) return false;
      this.applyAuth(auth);
      if (auth.state !== "authenticated") return false;
      this.loading = true;
      await this.loadOnboarding();
      return generation === this.generation && authVersion === this.authVersion;
    } catch (error) {
      if (generation !== this.generation || authVersion !== this.authVersion) return false;
      this.authError = this.message(error);
      if (error instanceof Error && error.message === "setup_closed") this.setupClosed = true;
      this.handleAuthError(error);
      return false;
    }
  };

  loginAuth = async (password: string): Promise<boolean> => {
    this.stop();
    const authVersion = this.authVersion;
    const generation = this.generation;
    this.authError = "";
    try {
      const auth = await api.auth.login(password);
      if (generation !== this.generation || authVersion !== this.authVersion) return false;
      this.applyAuth(auth);
      if (auth.state !== "authenticated") return false;
      this.loading = true;
      await this.loadOnboarding();
      return generation === this.generation && authVersion === this.authVersion;
    } catch (error) {
      if (generation !== this.generation || authVersion !== this.authVersion) return false;
      this.authError = this.message(error);
      this.handleAuthError(error);
      return false;
    }
  };

  logoutAuth = async (): Promise<void> => {
    this.stop();
    const authVersion = this.authVersion;
    const generation = this.generation;
    this.authLoading = true;
    const pending = api.auth.logout();
    this.currentStatus = null;
    this.authState = "login";
    this.auth = null;
    this.onboarding = null;
    this.actionError = "";
    this.actionWarning = "";
    this.authError = "";
    try {
      const auth = await pending;
      if (generation !== this.generation || authVersion !== this.authVersion) return;
      this.applyAuth(auth);
    } catch (error) {
      if (generation !== this.generation || authVersion !== this.authVersion) return;
      this.authError = this.message(error);
      this.handleAuthError(error);
    } finally {
      if (generation === this.generation && authVersion === this.authVersion) this.authLoading = false;
    }
  };

  changePassword = async (currentPassword: string, password: string): Promise<boolean> => {
    if (this.busy || this.authState !== "authenticated") return false;
    const generation = this.generation;
    const authVersion = ++this.authVersion;
    this.authLoading = false;
    this.mutation = "password";
    try {
      const auth = await api.auth.changePassword(currentPassword, password);
      if (generation !== this.generation || authVersion !== this.authVersion) return false;
      if (auth.state !== "authenticated") throw new ApiError("Unexpected password reply");
      // Rotation keeps the same app/poll lifecycle and onboarding in flight.
      this.auth = auth;
      this.authState = auth.state;
      setCsrfToken(auth.csrf_token);
      this.authError = "";
      return true;
    } catch (error) {
      if (generation !== this.generation || authVersion !== this.authVersion) return false;
      const uncertain = error instanceof ApiError && (error.status === undefined || error.status >= 500);
      const message = uncertain
        ? "Результат смены пароля неизвестен. Запрос не повторён автоматически. Обновите страницу; если потребуется вход, попробуйте новый пароль."
        : this.message(error);
      if (this.handleAuthError(error) || uncertain) this.authError = message;
      throw new Error(message);
    } finally {
      if (generation === this.generation && authVersion === this.authVersion) this.mutation = null;
    }
  };

  requireFreshStatus = (): boolean => {
    if (!this.statusUncertain) return true;
    this.actionError = refreshRequired;
    return false;
  };

  setMode = async (vpnEnabled: boolean): Promise<boolean> => {
    if (this.busy) return false;
    if (!this.requireFreshStatus()) return false;
    if (
      this.currentStatus?.vpn_enabled === vpnEnabled &&
      (!vpnEnabled || this.currentStatus.tunnel_active)
    ) {
      return true;
    }
    return this.mutate("mode", () => api.mode.set(vpnEnabled));
  };

  startUpdate = async (): Promise<void> => {
    await this.mutate("update", api.update.start);
  };

  setAutoUpdate = (enabled: boolean): Promise<boolean> =>
    this.mutate("auto-update", () => api.update.setAuto(enabled));

  importServer = (input: ProfileInput): Promise<boolean> =>
    this.mutate("import-server", () => api.servers.import(input));

  updateServer = (
    previousPublicKey: string,
    input: ServerInput,
  ): Promise<boolean> =>
    this.mutate(`edit:${previousPublicKey}`, () =>
      api.servers.update(previousPublicKey, input),
    );

  selectServer = (publicKey: string): Promise<boolean> => {
    if (this.busy) return Promise.resolve(false);
    if (!this.requireFreshStatus()) return Promise.resolve(false);
    if (publicKey === this.currentStatus?.active_server_key) {
      return Promise.resolve(false);
    }
    return this.mutate(`select:${publicKey}`, () =>
      api.servers.select(publicKey),
    );
  };

  removeServer = (publicKey: string): Promise<boolean> =>
    this.mutate(`delete:${publicKey}`, () =>
      api.servers.remove(publicKey),
    );

  bootstrapServer = (
    name: string,
    host: string,
    port: number,
    password: string,
    onStage: (stage: BootstrapStage) => void,
  ): Promise<boolean> => {
    const generation = this.generation;
    return this.mutate("bootstrap-server", () =>
      api.servers.bootstrap(name, host, port, password, stage => {
        if (generation === this.generation) onStage(stage);
      }),
    );
  };

  checkServer = (publicKey: string): Promise<ServerVersion | null> =>
    this.mutateResult(`check:${publicKey}`, () => api.servers.check(publicKey));

  updateManagedServer = (publicKey: string): Promise<ServerVersion | null> =>
    this.mutateResult(`update:${publicKey}`, () =>
      api.servers.updateManaged(publicKey),
    );

  createFriendProfile = (publicKey: string): Promise<Profile | null> =>
    this.mutateResult(`profile:${publicKey}`, () =>
      api.servers.createProfile(publicKey),
    );

  inspectServer = async (publicKey: string): Promise<ManagedServerStatus | null> => {
    const generation = this.generation;
    const result = await this.mutateResult(`inspect:${publicKey}`, () => api.servers.inspect(publicKey));
    if (generation !== this.generation) return null;
    if (result && this.warningServerKey === publicKey) this.actionWarning = "";
    return result;
  };

  restartServer = (publicKey: string): Promise<ManagedServerStatus | null> =>
    this.mutateResult(`restart:${publicKey}`, () => api.servers.restart(publicKey));

  createFriend = (publicKey: string, name: string): Promise<ManagedServerStatus | null> =>
    this.mutateResult(`friend-create:${publicKey}`, () => api.servers.createFriend(publicKey, name));

  renameFriend = (publicKey: string, peerKey: string, name: string): Promise<ManagedServerStatus | null> =>
    this.mutateResult(`friend-rename:${publicKey}`, () => api.servers.renameFriend(publicKey, peerKey, name));

  revokeFriend = (publicKey: string, peerKey: string): Promise<ManagedServerStatus | null> =>
    this.mutateResult(`friend-revoke:${publicKey}`, () => api.servers.revokeFriend(publicKey, peerKey));

  friendProfile = (publicKey: string, peerKey: string): Promise<Profile | null> =>
    this.mutateResult(`friend-profile:${publicKey}`, () => api.servers.friendProfile(publicKey, peerKey));

  saveRouting = (input: RoutingConfig): Promise<boolean> =>
    this.mutate("routing", () => api.routing.save(input));

  refreshLanDevices = async (): Promise<void> => {
    if (this.inventoryLoading || this.authState !== "authenticated") return;
    const generation = this.generation;
    const authVersion = this.authVersion;
    this.inventoryLoading = true;
    try {
      const inventory = await api.lanDevices.get();
      if (generation !== this.generation || authVersion !== this.authVersion) return;
      this.lanDevices = inventory;
      this.inventoryError = "";
    } catch (error) {
      if (generation !== this.generation || authVersion !== this.authVersion || this.handleAuthError(error)) return;
      this.lanDevices = { ...this.lanDevices, discovery: "unavailable" };
      this.inventoryError = this.message(error);
    } finally {
      if (generation === this.generation) this.inventoryLoading = false;
    }
  };

  setDeviceExcluded = async (input: DeviceExclusionInput): Promise<boolean> => {
    if (this.busy || this.authState !== "authenticated") return false;
    if (!this.hasStatus || this.pollError || !this.requireFreshStatus()) {
      this.actionError = refreshRequired;
      return false;
    }
    const parsed = deviceExclusionInputSchema.safeParse(input);
    if (!parsed.success) { this.actionError = parsed.error.issues[0].message; return false; }
    const { mac, excluded } = parsed.data;
    const saved = this.status.device_exclusions;
    if (excluded && !saved.includes(mac) && !deviceExclusionsSchema.safeParse([...saved, mac]).success) {
      this.actionError = "Можно сохранить не более 256 устройств.";
      return false;
    }
    const ok = await this.mutate(`device-exclusion:${mac}`, () => api.deviceExclusions.set(parsed.data));
    return ok && !this.statusUncertain;
  };

  testRouting = async (value: string): Promise<RoutingTest> => {
    const generation = this.generation;
    const authVersion = this.authVersion;
    try {
      const result = await api.routing.test(value);
      if (generation !== this.generation || authVersion !== this.authVersion) throw new Error("Request superseded");
      return result;
    } catch (error) {
      if (generation === this.generation && authVersion === this.authVersion) this.handleAuthError(error);
      throw error;
    }
  };

  completeOnboarding = async (): Promise<boolean> => {
    const generation = this.generation;
    const result = await this.mutateResult("onboarding-complete", api.onboarding.complete);
    if (generation !== this.generation || !result) return false;
    this.onboarding = result;
    this.startPolling();
    return true;
  };
}
