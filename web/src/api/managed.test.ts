import { friendNameSchema, managedServerStatusSchema, serverInputSchema, serverSchema } from "./schemas";

declare function test(name: string, run: () => void): void;

test("managed peer contracts reject inconsistent state and retain server icons", () => {
  const public_key = `${"A".repeat(43)}=`;
  const peer = { public_key, name: "Friend", revoked: false, can_share: true };
  const status = { version: "0.5.14", peers: [peer] };
  if (!managedServerStatusSchema.safeParse(status).success
    || managedServerStatusSchema.safeParse({ ...status, peers: [peer, peer] }).success
    || managedServerStatusSchema.safeParse({ ...status, peers: [{ ...peer, revoked: true }] }).success
    || friendNameSchema.safeParse("friend\nname").success
    || friendNameSchema.safeParse("a".repeat(61)).success
    || !friendNameSchema.safeParse("friend").success) throw new Error("invalid friend-management contract");
  const server = { name: "Server", endpoint: "203.0.113.1:8443", public_key, managed: true };
  if (!serverSchema.safeParse(server).success
    || serverInputSchema.parse({ ...server, emoji: "" }).emoji !== "") throw new Error("server icon update was discarded");
});
