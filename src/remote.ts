// What a window that controls another device needs to know without asking it.
// The device decides every request itself. These rules only let the interface say why a control is off.

import type { DeviceKind } from "./desktop";

/** The version of the request set that two devices speak. It must equal `PROTOCOL` in `dispatch.rs`. */
export const PROTOCOL = 1;

export type Session = {
  device: { id: string; name: string; kind: DeviceKind };
  /** What this device may do on the other one. */
  permissions: { control: boolean; applications: boolean; actions: boolean };
  version: string;
  protocol: number;
};
/** The language and theme of the person at this window. They never come from the controlled device. */
export type Viewer = { language: "en" | "es" | "system"; theme: "system" | "light" | "dark" };
/** `localOnly` means only the person at that device may do it. `notPermitted` means it did not allow this device. */
export type Reason = "localOnly" | "notPermitted";

/** The errors that say the link to the device, not the request, failed. The window shows them as one banner. */
const linkErrors = new Set([
  "sshUnreachable", "sshDenied", "sshTimeout", "sshFailed", "sshHostKeyChanged", "sshHostKeyUnknown",
  "sshOutputTooLarge", "sshInputTooLarge", "appNotRunning", "remoteUnavailable", "notPaired", "busy",
  "responseTooLong", "remoteFailed", "lineTooLong",
]);
/** The link errors that do not end by themselves: the user must act on one of the devices. */
const refusedErrors = new Set(["sshDenied", "sshHostKeyChanged", "sshHostKeyUnknown", "notPaired"]);

export function isLinkError(error: unknown): error is string {
  return typeof error === "string" && linkErrors.has(error);
}

export function isRefusedLink(error: unknown): boolean {
  return typeof error === "string" && refusedErrors.has(error);
}

/** The requests that only the person at the device may send. Keep it equal to `Access::LocalOnly` in `dispatch.rs`. */
const localOnly = new Set([
  "setProxyRole", "setProxyPort", "setProxyServer", "saveProxyApps", "saveProxyAllowed", "getProxyPeers", "getProxyLog",
  "requestActivityAccessibility",
]);
/** The requests that need the Control permission. Only the ones that a control of this window sends. */
const needsControl = new Set([
  "setToolActive", "setQuickStart", "setDuration", "setSchedule", "saveCaffeinateSettings", "setLidMode",
  "setStartToolsOnLaunch", "setHideWhileSharing", "setLoginLaunch",
]);

/** The requests that need the Applications permission. Keep it equal to `Access::Applications` in `dispatch.rs`. */
const needsApplications = new Set(["listApplications", "getAppIcon", "openApplication"]);

/** Why the device will refuse a request, when the window can know it before it asks. */
export function refusal(
  command: string, args: { tool?: string; active?: boolean } | undefined, permissions: Session["permissions"] | undefined,
): Reason | undefined {
  if (localOnly.has(command)) return "localOnly";
  // Starting the proxy server opens a port on the device. Stopping it does not.
  if (command === "setToolActive" && args?.tool === "proxy" && args.active) return "localOnly";
  if (permissions && !permissions.control && needsControl.has(command)) return "notPermitted";
  if (permissions && !permissions.applications && needsApplications.has(command)) return "notPermitted";
}

/** The refusals that the interface shows as a calm note instead of a failure. */
export function isRefusal(error: unknown): error is Reason {
  return error === "localOnly" || error === "notPermitted";
}
