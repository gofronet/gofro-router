import { api, ApiError } from "../api";
import { clearCsrfToken, setCsrfToken } from "../api/client";
import { modeService } from "../features/mode/service";
import { routingService } from "../features/routing/service";
import { serverService } from "../features/servers/service";
import { statusService } from "../features/status/service";
import { wifiService } from "../features/wifi/service";
import type {
  ProfileInput,
  AuthStatus,
  OnboardingStatus,
  OnboardingWifiInput,
  RoutingConfig,
  RoutingTest,
  ServerInput,
  ServerProbe,
  ServerVersion,
  Profile,
  Status,
  WifiBand,
} from "../domain/models";

export class RouterState {
  private currentStatus = $state<Status | null>(null);
  private pollInFlight = false;
  private statusVersion = 0;
  private interval: number | null = null;
  private onboardingInterval: number | null = null;
  private onboardingInFlight = false;
  private onboardingVersion = 0;

  loading = $state(true);
  authLoading = $state(true);
  authState = $state<AuthStatus["state"]>("login");
  authError = $state("");
  setupClosed = $state(false);
  onboardingLoading = $state(false);
  onboarding = $state<OnboardingStatus | null>(null);
  pollError = $state("");
  actionError = $state("");
  mutation = $state<string | null>(null);
  reconnectSsid = $state<string | null>(null);

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

  private message(error: unknown): string {
    if (!(error instanceof Error)) return "Неизвестная ошибка";
    switch (error.message) {
      case "password_too_short": return "Пароль администратора должен содержать не менее 12 символов.";
      case "password_too_long": return "Пароль слишком длинный. Максимум 128 байт: кириллица занимает больше одного байта на символ.";
      case "invalid_setup_code": return "Неверный пароль Wi-Fi. Введите текущий пароль сети этого роутера.";
      case "setup_unavailable": return "Не удалось прочитать пароль Wi-Fi на роутере. Требуется проверка настройки устройства.";
      case "invalid_password": return "Неверный пароль администратора.";
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

  get setupMethod(): "local" | "wifi_password" | null {
    return this.auth?.state === "setup" ? this.auth.setup_method : null;
  }

  get setupWindowSeconds(): number | null {
    return this.auth?.state === "setup" && this.auth.setup_method === "local"
      ? this.auth.setup_window_seconds
      : null;
  }

  private handleAuthError = (error: unknown): boolean => {
    if (!(error instanceof ApiError) || error.status !== 401) return false;
    this.onboardingVersion++;
    this.statusVersion++;
    this.currentStatus = null;
    this.loading = false;
    this.authState = "login";
    this.auth = null;
    this.stopPolling();
    this.stopOnboardingPolling();
    this.onboarding = null;
    return true;
  };

  private mutateResult = async <T>(
    kind: string,
    operation: () => Promise<T>,
  ): Promise<T | null> => {
    if (this.busy) return null;

    this.statusVersion++;
    this.mutation = kind;
    this.actionError = "";

    try {
      const result = await operation();
      this.pollError = "";
      return result;
    } catch (error) {
      this.handleAuthError(error);
      this.actionError = this.message(error);
      return null;
    } finally {
      this.mutation = null;
    }
  };

  private mutate = async (
    kind: string,
    operation: () => Promise<Status>,
  ): Promise<boolean> => {
    const result = await this.mutateResult(kind, operation);
    if (!result) return false;
    this.currentStatus = result;
    return true;
  };

  refresh = async (): Promise<void> => {
    if (this.pollInFlight || this.mutation || this.reconnectSsid) return;

    this.pollInFlight = true;
    const version = this.statusVersion;

    try {
      const nextStatus = await statusService.get();
      if (version === this.statusVersion) {
        this.currentStatus = nextStatus;
        this.pollError = "";
      }
    } catch (error) {
      if (this.handleAuthError(error)) return;
      if (version === this.statusVersion) {
        this.pollError = this.message(error);
      }
    } finally {
      this.loading = false;
      this.pollInFlight = false;
    }
  };

  startPolling(): void {
    if (this.interval !== null) return;
    void this.refresh();
    this.interval = window.setInterval(this.refresh, 5_000);
  }

  stopPolling(): void {
    if (this.interval === null) return;
    window.clearInterval(this.interval);
    this.interval = null;
  }

  stop(): void {
    this.onboardingVersion++;
    this.stopPolling();
    this.stopOnboardingPolling();
  }

  private startOnboardingPolling(): void {
    if (this.onboardingInterval !== null) return;
    this.onboardingInterval = window.setInterval(() => void this.loadOnboarding(), 5_000);
  }

  private stopOnboardingPolling(): void {
    if (this.onboardingInterval === null) return;
    window.clearInterval(this.onboardingInterval);
    this.onboardingInterval = null;
  }

  loadOnboarding = async (): Promise<void> => {
    if (this.authState !== "authenticated" || this.onboardingInFlight) return;
    const version = this.onboardingVersion;
    this.onboardingInFlight = true;
    this.onboardingLoading = true;
    try {
      const onboarding = await api.onboarding.get();
      if (version !== this.onboardingVersion || this.authState !== "authenticated") return;
      this.onboarding = {
        ...onboarding,
        error: onboarding.error === "onboarding_failed"
          ? "Не удалось применить Wi-Fi. Проверьте настройки и попробуйте снова."
          : onboarding.error,
      };
      if (onboarding.step === "wifi_applying") this.startOnboardingPolling();
      else {
        this.stopOnboardingPolling();
        this.reconnectSsid = null;
      }
      if (onboarding.step !== "complete") this.stopPolling();
      if (onboarding.step === "server" && !this.hasStatus) await this.refresh();
      if (onboarding.step === "complete") this.startPolling();
    } catch (error) {
      if (version !== this.onboardingVersion || this.authState !== "authenticated") return;
      this.handleAuthError(error);
      this.actionError = this.message(error);
    } finally {
      if (version === this.onboardingVersion) this.onboardingLoading = false;
      this.onboardingInFlight = false;
    }
  };

  clearActionError = (): void => {
    this.actionError = "";
  };

  initializeAuth = async (): Promise<void> => {
    this.authLoading = true;
    this.authError = "";
    try {
      const auth = await api.auth.status();
      this.applyAuth(auth);
      this.loading = auth.state === "authenticated";
      if (auth.state === "authenticated") await this.loadOnboarding();
    } catch (error) {
      clearCsrfToken();
      this.authState = "login";
      this.auth = null;
      this.authError = this.message(error);
      this.loading = false;
    } finally {
      this.authLoading = false;
    }
  };

  setupAuth = async (password: string, setupCode?: string): Promise<boolean> => {
    this.authError = "";
    try {
      const auth = await api.auth.setup(password, setupCode);
      this.applyAuth(auth);
      if (auth.state !== "authenticated") return false;
      this.loading = true;
      await this.loadOnboarding();
      return true;
    } catch (error) {
      this.authError = this.message(error);
      if (error instanceof Error && error.message === "setup_closed") this.setupClosed = true;
      this.handleAuthError(error);
      return false;
    }
  };

  loginAuth = async (password: string): Promise<boolean> => {
    this.authError = "";
    try {
      const auth = await api.auth.login(password);
      this.applyAuth(auth);
      if (auth.state !== "authenticated") return false;
      this.loading = true;
      await this.loadOnboarding();
      return true;
    } catch (error) {
      this.authError = this.message(error);
      this.handleAuthError(error);
      return false;
    }
  };

  logoutAuth = async (): Promise<void> => {
    try {
      this.applyAuth(await api.auth.logout());
    } catch (error) {
      if (!this.handleAuthError(error)) this.actionError = this.message(error);
      return;
    }
    this.currentStatus = null;
    this.authState = "login";
    this.auth = null;
    this.stop();
    this.onboarding = null;
  };

  setMode = async (vpnEnabled: boolean): Promise<void> => {
    if (
      this.currentStatus?.vpn_enabled === vpnEnabled &&
      (!vpnEnabled || this.currentStatus.tunnel_active)
    ) {
      return;
    }
    await this.mutate("mode", () => modeService.set(vpnEnabled));
  };

  startUpdate = async (): Promise<void> => {
    await this.mutate("update", api.update.start);
  };

  importServer = (input: ProfileInput): Promise<boolean> =>
    this.mutate("import-server", () => serverService.import(input));

  updateServer = (
    previousPublicKey: string,
    input: ServerInput,
  ): Promise<boolean> =>
    this.mutate(`edit:${previousPublicKey}`, () =>
      serverService.update(previousPublicKey, input),
    );

  selectServer = (publicKey: string): Promise<boolean> => {
    if (publicKey === this.currentStatus?.active_server_key) {
      return Promise.resolve(false);
    }
    return this.mutate(`select:${publicKey}`, () =>
      serverService.select(publicKey),
    );
  };

  removeServer = (publicKey: string): Promise<boolean> =>
    this.mutate(`delete:${publicKey}`, () =>
      serverService.remove(publicKey),
    );

  probeServer = (host: string, port: number): Promise<ServerProbe | null> =>
    this.mutateResult("probe-server", () => serverService.probe(host, port));

  bootstrapServer = (
    name: string,
    host: string,
    port: number,
    password: string,
    hostKey: string,
  ): Promise<boolean> =>
    this.mutate("bootstrap-server", () =>
      serverService.bootstrap(name, host, port, password, hostKey),
    );

  checkServer = (publicKey: string): Promise<ServerVersion | null> =>
    this.mutateResult(`check:${publicKey}`, () => serverService.check(publicKey));

  updateManagedServer = (publicKey: string): Promise<ServerVersion | null> =>
    this.mutateResult(`update:${publicKey}`, () =>
      serverService.updateManaged(publicKey),
    );

  createFriendProfile = (publicKey: string): Promise<Profile | null> =>
    this.mutateResult(`profile:${publicKey}`, () =>
      serverService.createProfile(publicKey),
    );

  saveAp = async (
    band: WifiBand | undefined,
    ssid: string,
    password: string,
  ): Promise<boolean> => {
    const saved = await this.mutate(`ap:${band ?? "all"}`, () =>
      wifiService.save({ band, ssid, password }),
    );
    if (saved) this.reconnectSsid = ssid;
    return saved;
  };

  saveRouting = (input: RoutingConfig): Promise<boolean> =>
    this.mutate("routing", () => routingService.save(input));

  testRouting = async (value: string): Promise<RoutingTest> => {
    try {
      return await routingService.test(value);
    } catch (error) {
      this.handleAuthError(error);
      throw error;
    }
  };

  resumePolling = async (): Promise<void> => {
    this.reconnectSsid = null;
    await this.refresh();
  };

  saveOnboardingWifi = async (
    input: OnboardingWifiInput,
  ): Promise<"saved" | "reconnect" | "error"> => {
    if (this.busy) return "error";
    this.stopPolling();
    this.mutation = "onboarding-wifi";
    this.actionError = "";
    try {
      this.onboarding = await api.onboarding.wifi(input);
      this.reconnectSsid = input.networks.map((network) => network.ssid).join(", ");
      this.startOnboardingPolling();
      return "saved";
    } catch (error) {
      if (error instanceof ApiError && error.status === undefined) {
        this.reconnectSsid = input.networks.map((network) => network.ssid).join(", ");
        this.startOnboardingPolling();
        return "reconnect";
      }
      this.handleAuthError(error);
      this.actionError = this.message(error);
      return "error";
    } finally {
      this.mutation = null;
    }
  };

  completeOnboarding = async (): Promise<boolean> => {
    const result = await this.mutateResult("onboarding-complete", api.onboarding.complete);
    if (!result) return false;
    this.onboarding = result;
    this.reconnectSsid = null;
    this.stopOnboardingPolling();
    this.startPolling();
    return true;
  };
}
