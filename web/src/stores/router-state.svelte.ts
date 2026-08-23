import { modeService } from "../features/mode/service";
import { serverService } from "../features/servers/service";
import { statusService } from "../features/status/service";
import { updateService } from "../features/update/service";
import { wifiService } from "../features/wifi/service";
import type { ServerInput, Status, UpdateStatus } from "../domain/models";

const activeUpdateStates = ["checking", "downloading", "installing"];

export class RouterState {
  private currentStatus = $state<Status | null>(null);
  private pollInFlight = false;
  private statusVersion = 0;
  private interval: number | null = null;
  private updatePollInFlight = false;
  private updaterVersion = 0;
  private updaterInterval: number | null = null;
  private observedInstalledVersion: string | null = null;

  loading = $state(true);
  pollError = $state("");
  actionError = $state("");
  mutation = $state<string | null>(null);
  reconnectSsid = $state<string | null>(null);
  updateStatus = $state<UpdateStatus | null>(null);
  updateError = $state("");
  updatePollError = $state("");
  updateAction = $state<"check" | "start" | null>(null);

  get status(): Status {
    if (!this.currentStatus) throw new Error("Status is not loaded yet");
    return this.currentStatus;
  }

  get hasStatus(): boolean {
    return this.currentStatus !== null;
  }

  get busy(): boolean {
    return (
      this.mutation !== null ||
      this.updateActive ||
      this.updateAction !== null
    );
  }

  get updateActive(): boolean {
    return (
      this.updateStatus !== null &&
      activeUpdateStates.includes(this.updateStatus.state)
    );
  }

  get updaterError(): string {
    return this.updateError || this.updatePollError;
  }

  get updateText(): string {
    if (!this.updateStatus) {
      return this.updaterError
        ? "Сервис обновлений недоступен"
        : "Получаем состояние обновлений";
    }
    return {
      idle: this.updateStatus.version
        ? "Установлена актуальная версия"
        : "Нажмите, чтобы проверить новую версию",
      checking: "Проверяем подписанный release",
      available: `Доступна версия ${this.updateStatus.version ?? ""}`,
      downloading: "Скачиваем и проверяем подпись",
      installing: "Устанавливаем и проверяем подключение",
      success: `Версия ${this.updateStatus.version ?? ""} успешно установлена`,
      error: this.updateStatus.message || "Обновление не выполнено",
    }[this.updateStatus.state];
  }

  private message(error: unknown): string {
    return error instanceof Error ? error.message : "Неизвестная ошибка";
  }

  private mutate = async (
    kind: string,
    operation: () => Promise<Status>,
  ): Promise<boolean> => {
    if (this.busy) return false;

    this.statusVersion++;
    this.mutation = kind;
    this.actionError = "";

    try {
      this.currentStatus = await operation();
      this.pollError = "";
      return true;
    } catch (error) {
      this.actionError = this.message(error);
      return false;
    } finally {
      this.mutation = null;
    }
  };

  refresh = async (): Promise<void> => {
    if (this.pollInFlight || this.mutation || this.reconnectSsid) return;

    this.pollInFlight = true;
    const version = this.statusVersion;

    try {
      const nextStatus = await statusService.get();
      if (version === this.statusVersion) {
        this.observedInstalledVersion ??= nextStatus.version;
        this.currentStatus = nextStatus;
        this.pollError = "";
      }
    } catch (error) {
      if (version === this.statusVersion) {
        this.pollError = this.message(error);
      }
    } finally {
      this.loading = false;
      this.pollInFlight = false;
    }
  };

  refreshUpdate = async (): Promise<void> => {
    if (this.updatePollInFlight || this.updateAction) return;

    this.updatePollInFlight = true;
    const version = this.updaterVersion;

    try {
      const next = await updateService.get();
      if (version === this.updaterVersion) {
        this.updateStatus = next;
        this.updateError = "";
        this.updatePollError = "";
        if (
          next.state === "success" &&
          this.observedInstalledVersion &&
          this.observedInstalledVersion !== next.installed_version
        ) {
          location.reload();
        }
        this.observedInstalledVersion ??= next.installed_version;
      }
    } catch (error) {
      if (version === this.updaterVersion) {
        this.updatePollError = this.message(error);
      }
    } finally {
      this.updatePollInFlight = false;
    }
  };

  startPolling(): void {
    if (this.interval !== null) return;
    void this.refresh();
    void this.refreshUpdate();
    this.interval = window.setInterval(this.refresh, 2_000);
    this.updaterInterval = window.setInterval(this.refreshUpdate, 1_000);
  }

  stopPolling(): void {
    if (this.interval === null) return;
    window.clearInterval(this.interval);
    this.interval = null;
    if (this.updaterInterval !== null) {
      window.clearInterval(this.updaterInterval);
      this.updaterInterval = null;
    }
  }

  clearActionError = (): void => {
    this.actionError = "";
  };

  private requestUpdate = async (
    kind: "check" | "start",
    operation: () => Promise<UpdateStatus>,
  ): Promise<boolean> => {
    if (this.updateAction || this.updateActive) return false;

    this.updaterVersion++;
    this.updateAction = kind;
    this.updateError = "";
    try {
      this.updateStatus = await operation();
      return true;
    } catch (error) {
      this.updateError = this.message(error);
      return false;
    } finally {
      this.updateAction = null;
    }
  };

  checkUpdate = (): Promise<boolean> =>
    this.requestUpdate("check", updateService.check);

  startUpdate = (): Promise<boolean> =>
    this.requestUpdate("start", updateService.start);

  setMode = async (vpnEnabled: boolean): Promise<void> => {
    if (
      this.currentStatus?.vpn_enabled === vpnEnabled &&
      (!vpnEnabled || this.currentStatus.tunnel_active)
    ) {
      return;
    }
    await this.mutate("mode", () => modeService.set(vpnEnabled));
  };

  createServer = (input: ServerInput): Promise<boolean> =>
    this.mutate("add-server", () => serverService.create(input));

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

  saveAp = async (ssid: string, password: string): Promise<boolean> => {
    const saved = await this.mutate("ap", () =>
      wifiService.save({ ssid, password }),
    );
    if (saved) this.reconnectSsid = ssid;
    return saved;
  };

  resumePolling = async (): Promise<void> => {
    this.reconnectSsid = null;
    await this.refresh();
  };
}
