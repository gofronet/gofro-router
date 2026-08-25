import { modeService } from "../features/mode/service";
import { routingService } from "../features/routing/service";
import { serverService } from "../features/servers/service";
import { statusService } from "../features/status/service";
import { wifiService } from "../features/wifi/service";
import type {
  RoutingConfig,
  RoutingTest,
  ServerInput,
  Status,
} from "../domain/models";

export class RouterState {
  private currentStatus = $state<Status | null>(null);
  private pollInFlight = false;
  private statusVersion = 0;
  private interval: number | null = null;

  loading = $state(true);
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

  clearActionError = (): void => {
    this.actionError = "";
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

  saveRouting = (input: RoutingConfig): Promise<boolean> =>
    this.mutate("routing", () => routingService.save(input));

  testRouting = (value: string): Promise<RoutingTest> =>
    routingService.test(value);

  resumePolling = async (): Promise<void> => {
    this.reconnectSsid = null;
    await this.refresh();
  };
}
