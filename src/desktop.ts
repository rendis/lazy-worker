import { getVersion } from "@tauri-apps/api/app";
import { convertFileSrc, invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { version as previewVersion } from "../src-tauri/tauri.conf.json";
import type { Language } from "./messages";
import type { ToolId } from "./modules";
import { isLinkError, PROTOCOL, refusal, type Session, type Viewer } from "./remote";
import claudeIcon from "./assets/apps/claude.svg";
import notesIcon from "./assets/apps/notes.svg";
import safariIcon from "./assets/apps/safari.svg";

export type Theme = "system" | "light" | "dark";
export type Duration = "minutes15" | "minutes30" | "hour" | "untilStopped";
export type CaffeinateFlag = "i" | "d" | "m" | "s" | "u";
export type CaffeinateSettings = {
  mode: "untilStopped" | "timer" | "process";
  flags: Record<CaffeinateFlag, boolean>;
  seconds: number;
  pid: number | null;
};
export type ProxyApp = {
  id: string;
  name: string;
  icon: string;
  /** `null` keeps the built-in command. `{server}` stands for the server address. */
  command: string | null;
  /** True when the app has its own command. A remote device gets `command: null` and only this flag. */
  custom?: boolean;
  quickStart: boolean;
  /** The desktop app or the command-line program of a built-in tool. */
  variant: "app" | "code";
};
export type ProxySettings = {
  role: "server" | "client";
  port: number;
  allowed: string[];
  server: string;
  apps: ProxyApp[];
};
export type ProxyNetwork = { hostname: string | null; addresses: string[] };
export type ProxyPeer = { address: string; name: string | null; connections: number; idleSeconds: number };
export type ProxyLogEntry = {
  id: number;
  /** Seconds since the Unix epoch. */
  time: number;
  peer: string;
  target: string;
  outcome: "ok" | "denied" | "blocked" | "unreachable" | "invalid";
};
export type LidMode = "sleep" | "charger" | "always";
/** `external` means the Mac already stays awake with its lid closed, and the app did not set that. */
export type LidStatus = { supported: boolean; external: boolean };
/** `checked` is the time of the last check that got an answer, in seconds since the Unix epoch. */
export type UpdateStatus = { version: string | null; checked: number | null };
export type ProxyInstalls = { claudeCode: boolean; claudeApp: boolean; codexCode: boolean; codexApp: boolean };
export type Preferences = {
  language: Language | "system";
  theme: Theme;
  duration: Duration;
  schedule: { enabled: boolean; start: string; end: string };
  quickStart: Record<ToolId, boolean>;
  startToolsOnLaunch: boolean;
  caffeinate: CaffeinateSettings;
  hideWhileSharing: boolean;
  proxy: ProxySettings;
  /** What the Mac does with its lid closed while Caffeinate is on. */
  lidAwake: LidMode;
  /** The version whose changes the user has seen. `null` until the setup or What’s new closes. */
  seenVersion: string | null;
};
export type Status = {
  mode: "off" | "manual" | "scheduled";
  supported: boolean;
  secondsLeft: number | null;
  elapsedSeconds: number | null;
  error: string | null;
};
export type Statuses = Record<ToolId, Status>;
export type ActivityAccessibility = "granted" | "required" | "unsupported" | "unavailable";

export type DeviceKind = "laptop" | "desktop";
export type ControllerPermission = "control" | "applications" | "actions" | "shell";
/** A device that may control this one. `lastAccess` is in seconds since the Unix epoch. */
export type Controller = {
  id: string;
  name: string;
  kind: DeviceKind;
  lastAccess: number | null;
  /** True while the device has an open connection. */
  connected: boolean;
  permissions: Record<ControllerPermission, boolean>;
};
/** One request or event in the log of remote access. `request` is a command name, never its arguments. */
export type RemoteLogEntry = {
  time: number;
  peer: string | null;
  name: string | null;
  request: string;
  /** `ok` or an error code. */
  outcome: string;
};
/** `weekday` counts from 0 on Sunday. `time` is a time of day as `HH:MM`. */
export type ActionSchedule =
  | { every: "manual" }
  | { every: "daily"; time: string }
  | { every: "weekly"; weekday: number; time: string };
/** A command saved on a device. Times are in seconds since the Unix epoch. */
export type SavedAction = {
  id: string;
  name: string;
  schedule: ActionSchedule;
  lastRun: { time: number; ok: boolean } | null;
  nextRun: number | null;
};
export type RemoteTool = { id: ToolId; running: boolean };
/** An application of another device. `bundleId` names it, and `version` is part of the name of its cached icon. */
export type RemoteApp = { bundleId: string; name: string; version: string; open: boolean };
export type RemoteDevice = {
  id: string;
  name: string;
  kind: DeviceKind;
  /** The address that pairing found. The account is the one the other device named. */
  user: string;
  host: string;
  port: number;
  /** Unix seconds when the pairing ended. */
  added: number;
  /** `null` until the first reachability check ends. */
  online: boolean | null;
  tools: RemoteTool[];
  /** The bundle identifiers of the apps the user chose to see first. The view reads the list itself. */
  apps: { pinned: string[] };
  actions: SavedAction[];
  /** The shell is on until this time, or `null` while it is off. Only that device can turn it on. */
  shell: { until: number } | null;
};
/** How this device lets others in over SSH. `macosMajor` is the major version of macOS. */
export type RemoteAccess = {
  remoteLogin: "off" | "on";
  passwordLogin: "blocked" | "allowed" | "unknown";
  /** `null` when the system gave no answer. */
  userAllowed: boolean | null;
  managedFile: boolean;
  macosMajor: number | null;
};
/** Whether this device lets another one pair with it. `expires` is Unix seconds. */
export type PairingState = { open: boolean; expires: number | null; attemptsLeft: number };
/** What this device shows while it waits for another one to pair. `account` is the name the other device types. */
export type OpenedPairing = { code: string; account: string; expires: number };
/** A device that announced an SSH server on this network. */
export type DiscoveredDevice = { name: string; host: string; port: number };
export type PairingPermissions = Pick<Controller["permissions"], "control" | "applications" | "actions">;
export type DevicesOverview = {
  thisDevice: {
    /** Empty when the system gives no name. */
    name: string;
    kind: DeviceKind;
    remoteAccess: RemoteAccess;
    pairing: PairingState;
    /** The user name on this device, as a first guess for the account on another one. */
    account: string | null;
    controllers: Controller[];
    /** Whether this device can receive requests, and an error code when it cannot. */
    socket: { listening: boolean; error: string | null };
    actions: SavedAction[];
  };
  devices: RemoteDevice[];
};

export const defaults: Preferences = {
  language: "system",
  theme: "system",
  duration: "minutes15",
  schedule: { enabled: false, start: "09:00", end: "17:00" },
  quickStart: { activity: false, caffeinate: false, proxy: false },
  startToolsOnLaunch: false,
  hideWhileSharing: true,
  lidAwake: "sleep",
  seenVersion: null,
  caffeinate: {
    mode: "untilStopped",
    flags: { i: false, d: false, m: false, s: true, u: false },
    seconds: 1800,
    pid: null,
  },
  proxy: {
    role: "server",
    port: 8899,
    allowed: [],
    server: "",
    apps: [
      { id: "claude", name: "Claude", icon: "sparkles", command: null, quickStart: false, variant: "app" },
      { id: "codex", name: "Codex", icon: "terminal", command: null, quickStart: false, variant: "app" },
    ],
  },
};

export const offStatus: Status = {
  mode: "off",
  supported: false,
  secondsLeft: null,
  elapsedSeconds: null,
  error: null,
};

export type RunningProcess = { pid: number; name: string };

type PreviewSession = {
  started: number;
  duration: number | null;
  caffeinate?: CaffeinateSettings;
};
const native = isTauri();
const previewQuery = new URLSearchParams(location.search);
let previewUpdate: UpdateStatus = { version: null, checked: null };
const macApp = native && navigator.userAgent.includes("Macintosh");
const previewKey = "lazy-worker-preview-preferences";
const previewSessions = new Map<ToolId, PreviewSession>();
const previewErrors = new Map<ToolId, string>();
const activityDurations: Record<Duration, number | null> = {
  minutes15: 900,
  minutes30: 1800,
  hour: 3600,
  untilStopped: null,
};

function loadPreviewPreferences(): Preferences {
  const stored = localStorage.getItem(previewKey);
  if (!stored) return defaults;
  const saved = JSON.parse(stored) as Omit<Partial<Preferences>, "caffeinate"> & {
    caffeinate?: Omit<Partial<CaffeinateSettings>, "mode"> & { mode?: CaffeinateSettings["mode"] | "command" };
  };
  const { mode, flags, seconds, pid } = saved.caffeinate ?? {};
  return {
    ...defaults,
    ...saved,
    quickStart: { ...defaults.quickStart, ...saved.quickStart },
    proxy: {
      ...defaults.proxy,
      ...saved.proxy,
      apps: (saved.proxy?.apps ?? defaults.proxy.apps).map(app => ({ ...app, variant: app.variant ?? "app" })),
    },
    caffeinate: {
      mode: !mode || mode === "command" ? defaults.caffeinate.mode : mode,
      flags: { ...defaults.caffeinate.flags, ...flags },
      seconds: seconds ?? defaults.caffeinate.seconds,
      pid: pid ?? null,
    },
  };
}

function savePreviewPreferences(preferences: Preferences): void {
  localStorage.setItem(previewKey, JSON.stringify(preferences));
}

/** Every native call goes through the one `dispatch` command, which decides the access in Rust. */
function call<T>(command: string, args?: object): Promise<T> {
  return invoke<T>("dispatch", { request: { command, args } }).then(
    value => {
      if (remoteId !== null) reportLink(undefined);
      return value;
    },
    (error: unknown) => {
      // In a window that controls a device, a failed link is a state of the window and not of the request.
      if (remoteId !== null && isLinkError(error)) reportLink(error);
      throw error;
    },
  );
}

/** A preview keeps a saved setting in local storage. Caffeinate and the lid choice have their own calls. */
function savePreview(change: (current: Preferences) => Preferences): void {
  savePreviewPreferences(change(loadPreviewPreferences()));
}

function previewStatus(id: ToolId): Status {
  const session = previewSessions.get(id);
  const error = previewErrors.get(id) ?? null;
  if (session) {
    const elapsed = (performance.now() - session.started) / 1000;
    if (session.duration === null || elapsed < session.duration) {
      return {
        mode: "manual",
        supported: true,
        elapsedSeconds: Math.floor(elapsed),
        secondsLeft: session.duration === null ? null : Math.ceil(session.duration - elapsed),
        error,
      };
    }
    previewSessions.delete(id);
  }
  return { ...offStatus, supported: true, error };
}

function previewStatuses(): Statuses {
  return { activity: previewStatus("activity"), caffeinate: previewStatus("caffeinate"), proxy: previewStatus("proxy") };
}

function previewCaffeinateError(settings: CaffeinateSettings): string | null {
  if (!Object.values(settings.flags).some(Boolean)) return "caffeinateNoFlags";
  const { mode, seconds, pid } = settings;
  if (mode === "timer" && (!Number.isInteger(seconds) || seconds < 1 || seconds > 4_294_967_295)) {
    return "caffeinateInvalidDuration";
  }
  if (mode === "process" && (pid === null || !Number.isInteger(pid) || pid < 1 || pid > 2_147_483_647)) {
    return "caffeinateInvalidPid";
  }
  return null;
}

function setPreviewActive(id: ToolId, active: boolean, preferences: Preferences): void {
  previewErrors.delete(id);
  if (!active) {
    previewSessions.delete(id);
    return;
  }
  if (previewStatus(id).mode !== "off") return;

  const settings = id === "caffeinate" ? preferences.caffeinate : undefined;
  const error = settings && previewCaffeinateError(settings);
  if (error) {
    previewErrors.set(id, error);
    return;
  }
  const duration = settings
    ? (settings.mode === "timer" ? settings.seconds : null)
    : activityDurations[preferences.duration];
  previewSessions.set(id, { started: performance.now(), duration, caffeinate: settings });
}

function updatePreviewCaffeinateSession(settings: CaffeinateSettings): void {
  if (previewStatus("caffeinate").mode === "off") return;
  const session = previewSessions.get("caffeinate");
  if (!session?.caffeinate) return;

  const current = session.caffeinate;
  const flagsChanged = (Object.keys(settings.flags) as CaffeinateFlag[])
    .some(flag => current.flags[flag] !== settings.flags[flag]);
  const restart = current.mode !== settings.mode
    || (settings.mode === "process" && current.pid !== settings.pid)
    || (settings.mode === "timer" && (current.seconds !== settings.seconds || flagsChanged));
  previewSessions.set("caffeinate", {
    started: restart ? performance.now() : session.started,
    duration: settings.mode === "timer" ? settings.seconds : null,
    caffeinate: settings,
  });
}

/** The controllers and the log of a preview. A change lasts until the page reloads. */
let previewControllers: Controller[] | undefined;
const previewPairing: PairingState = { open: false, expires: null, attemptsLeft: 0 };
/** A preview shows a device pairing with this one with `?pair-in`, six seconds after the window opens. */
let previewPairingOpened = 0;
/** The window ends by itself, as on the device. */
function settlePreviewPairing(now: number): void {
  if (!previewPairing.open) return;
  if (previewQuery.has("pair-in") && now - previewPairingOpened >= 6) {
    previewControllerList().push({
      id: "mini", name: "Mac mini de la sala", kind: "desktop", lastAccess: null, connected: false,
      permissions: { control: true, applications: false, actions: false, shell: false },
    });
    Object.assign(previewPairing, { open: false, expires: null, attemptsLeft: 0 });
  } else if (previewPairing.expires !== null && previewPairing.expires <= now) {
    Object.assign(previewPairing, { open: false, expires: null, attemptsLeft: 0 });
  }
}
/** Devices that a preview paired with, so the list shows them. */
const previewDeviceAdditions: RemoteDevice[] = [];
let previewLog: RemoteLogEntry[] | undefined;

function sampleControllers(now: number): Controller[] {
  return [
    {
      id: "studio", name: "iMac del estudio", kind: "desktop", lastAccess: now - 5 * 60, connected: true,
      permissions: { control: true, applications: true, actions: true, shell: false },
    },
    {
      id: "office", name: "MacBook Air de la oficina", kind: "laptop", lastAccess: now - 26 * 3600, connected: false,
      permissions: { control: true, applications: false, actions: false, shell: false },
    },
    {
      id: "new", name: "Mac mini", kind: "desktop", lastAccess: null, connected: false,
      permissions: { control: false, applications: false, actions: false, shell: false },
    },
  ];
}

function previewEntries(now: number): RemoteLogEntry[] {
  const studio = { peer: "studio", name: "iMac del estudio" };
  const office = { peer: "office", name: "MacBook Air de la oficina" };
  return [
    { time: now - 20, ...studio, request: "getStatuses", outcome: "ok" },
    { time: now - 95, ...studio, request: "setToolActive", outcome: "ok" },
    { time: now - 240, ...office, request: "getDevices", outcome: "localOnly" },
    { time: now - 3 * 3600, ...office, request: "launchProxyApp", outcome: "notPermitted" },
    { time: now - 26 * 3600, ...office, request: "getPreferences", outcome: "ok" },
    { time: now - 2 * 86400, ...studio, request: "getStatuses", outcome: "ok" },
  ];
}

function previewControllerList(): Controller[] {
  previewControllers ??= sampleControllers(Math.floor(Date.now() / 1000));
  return previewControllers;
}

function previewLogList(): RemoteLogEntry[] {
  previewLog ??= previewEntries(Math.floor(Date.now() / 1000));
  return previewLog;
}

/** A preview shows the remote access state with `?remote=off|password|keys|nouser`. The actions change it until the page reloads. */
const previewStates: Record<string, RemoteAccess> = {
  off: { remoteLogin: "off", passwordLogin: "unknown", userAllowed: null, managedFile: false, macosMajor: 26 },
  password: { remoteLogin: "on", passwordLogin: "allowed", userAllowed: true, managedFile: false, macosMajor: 26 },
  keys: { remoteLogin: "on", passwordLogin: "blocked", userAllowed: true, managedFile: true, macosMajor: 26 },
  nouser: { remoteLogin: "on", passwordLogin: "blocked", userAllowed: false, managedFile: true, macosMajor: 26 },
};
const previewAccess: RemoteAccess = { ...(previewStates[previewQuery.get("remote") ?? ""] ?? previewStates.keys) };
/** `?macos=12` shows the guide of macOS 12, and `?remote-fails` makes turning on fail until the user does it in Sharing. */
if (previewQuery.has("macos")) previewAccess.macosMajor = Number(previewQuery.get("macos")) || 26;

/**
 * The device this window controls, or `null` in the panel. A native window must be `remote-<id>` for the id in its address,
 * so a page of the panel cannot pose as one. A preview takes any name that is not an access state: `?remote=studio`.
 * `?lost` drops the link after a moment, `?mismatch` gives the device another version, and `?perm=none|control` limits it.
 */
const remoteParam = previewQuery.get("remote");
const remoteId: string | null = (() => {
  if (!remoteParam || remoteParam in previewStates) return null;
  if (!native) return remoteParam;
  return getCurrentWindow().label === `remote-${remoteParam}` && /^[0-9a-f]{32}$/.test(remoteParam) ? remoteParam : null;
})();
const linkListeners = new Set<(error: string | undefined) => void>();
let linkState: string | undefined;
function reportLink(error: string | undefined): void {
  if (error === linkState) return;
  linkState = error;
  linkListeners.forEach(listener => listener(error));
}
/** A preview link drops two and a half seconds after the window opens when the address has `?lost`. */
function previewLinkDown(): boolean {
  return previewQuery.has("lost") && performance.now() > 2500;
}
function previewPermissions(): Session["permissions"] {
  const level = previewQuery.get("perm");
  // `?no-apps` shows a device that has not allowed applications.
  return {
    control: level !== "none", applications: !previewQuery.has("no-apps"),
    actions: level !== "none" && level !== "control",
  };
}
/** What a window that controls a device finds out before a call, as the device would answer it. */
function previewGate(command: string, args?: { tool?: string; active?: boolean }): void {
  if (remoteId === null) return;
  if (previewLinkDown()) {
    reportLink("sshUnreachable");
    throw "sshUnreachable";
  }
  reportLink(undefined);
  const reason = refusal(command, args, previewPermissions());
  if (reason) throw reason;
}
/** The tools that a preview shows on another device. A change lasts until the page reloads. */
const previewRemoteRunning = new Map<string, Set<ToolId>>();

/** The applications of the sample device. A preview has real icons for three of them. */
const previewApps: { bundleId: string; name: string; version: string; icon?: string }[] = [
  ["com.apple.Safari", "Safari", "17.4", safariIcon], ["com.apple.Notes", "Notes", "4.11", notesIcon],
  ["com.anthropic.claudefordesktop", "Claude", "0.9.2", claudeIcon], ["com.apple.Maps", "Maps", "3.0"],
  ["com.apple.mail", "Mail", "16.0"], ["com.apple.iCal", "Calendar", "14.0"], ["com.apple.Photos", "Photos", "9.0"],
  ["com.apple.Music", "Music", "1.4"], ["com.apple.TV", "TV", "1.4"], ["com.apple.podcasts", "Podcasts", "1.1"],
  ["com.apple.reminders", "Reminders", "7.0"], ["com.apple.freeform", "Freeform", "2.0"],
  ["com.apple.Preview", "Preview", "11.0"], ["com.apple.TextEdit", "TextEdit", "1.19"],
  ["com.apple.calculator", "Calculator", "11.0"], ["com.apple.systempreferences", "System Settings", "15.0"],
  ["com.apple.Terminal", "Terminal", "2.14"], ["com.apple.ActivityMonitor", "Activity Monitor", "10.14"],
  ["com.apple.finder", "Finder", "15.0"], ["com.apple.dt.Xcode", "Xcode", "16.1"],
  ["com.microsoft.VSCode", "Visual Studio Code with an extremely long name that has to wrap", "1.95"],
  ["org.mozilla.firefox", "Firefox", "132.0"], ["com.spotify.client", "Spotify", "1.2"],
  ["com.tinyspeck.slackmacgap", "Slack", "4.40"], ["us.zoom.xos", "zoom.us", "6.2"],
  ["com.figma.Desktop", "Figma", "124.0"], ["com.docker.docker", "Docker", "4.35"],
  ["com.rendis.lazyworker", "Lazy Worker", previewVersion],
].map(([bundleId, name, version, icon]) => ({ bundleId, name, version, icon }));
const previewOpen = new Set(["com.apple.Safari", "com.apple.finder", "com.anthropic.claudefordesktop"]);
const previewPinned = ["com.apple.Safari", "com.apple.Notes", "com.anthropic.claudefordesktop"];

/** The sample list a preview shows, so the whole Devices view can be checked. Times follow the clock. */
function previewDevices(): DevicesOverview {
  const now = Math.floor(Date.now() / 1000);
  settlePreviewPairing(now);
  const tomorrow = new Date();
  tomorrow.setDate(tomorrow.getDate() + 1);
  tomorrow.setHours(2, 0, 0, 0);
  const shellEnd = new Date();
  shellEnd.setHours(shellEnd.getHours() + 3, 0, 0, 0);
  const backup: SavedAction = {
    id: "backup", name: "Backup", schedule: { every: "daily", time: "02:00" },
    lastRun: { time: now - 8 * 3600, ok: true }, nextRun: Math.floor(tomorrow.getTime() / 1000),
  };
  const docker: SavedAction = {
    id: "docker", name: "Restart Docker", schedule: { every: "manual" }, lastRun: null, nextRun: null,
  };
  return {
    thisDevice: {
      name: "MacBook Pro", kind: "laptop", remoteAccess: { ...previewAccess }, pairing: { ...previewPairing },
      account: "alex",
      controllers: previewControllerList().map(controller => ({
        ...controller, permissions: { ...controller.permissions },
      })),
      socket: { listening: true, error: null },
      actions: [
        {
          id: "xcode", name: "Clear the Xcode cache", schedule: { every: "weekly", weekday: 1, time: "09:00" },
          lastRun: { time: now - 2 * 86400, ok: false }, nextRun: null,
        },
        { id: "downloads", name: "Clean downloads", schedule: { every: "manual" }, lastRun: null, nextRun: null },
      ],
    },
    devices: [
      {
        id: "studio", name: "iMac del estudio", kind: "desktop", online: true,
        user: "studio", host: "imac-del-estudio.local", port: 22, added: now - 12 * 86400,
        tools: [{ id: "activity", running: true }, { id: "caffeinate", running: false }],
        apps: { pinned: [...previewPinned] },
        actions: [backup, docker],
        shell: { until: Math.floor(shellEnd.getTime() / 1000) },
      },
      {
        id: "old", name: "MacBook Air de la oficina con un nombre muy largo para probar el ajuste", kind: "laptop",
        user: "oficina", host: "192.168.1.40", port: 22, added: now - 90 * 86400,
        online: false, tools: [], apps: { pinned: [] }, actions: [], shell: null,
      },
      ...previewDeviceAdditions.map(device => ({ ...device })),
    ],
  };
}

/** The tools of a sample device, as a preview shows them in the Devices view. */
function previewRemoteStatuses(id: string): Statuses {
  const device = previewDevices().devices.find(item => item.id === id);
  const running = previewRemoteRunning.get(id)
    ?? new Set(device?.tools.filter(tool => tool.running).map(tool => tool.id));
  previewRemoteRunning.set(id, running);
  const status = (tool: ToolId): Status => ({
    ...offStatus, supported: true, mode: running.has(tool) ? "manual" : "off",
    elapsedSeconds: running.has(tool) ? 600 : null,
  });
  return { activity: status("activity"), caffeinate: status("caffeinate"), proxy: status("proxy") };
}

export const desktop = {
  /** The device that this window controls, or `null` in the panel. */
  remote: remoteId === null ? null : { id: remoteId },
  /** Calls `listener` with an error code when the link to the controlled device fails, and with nothing when it works again. */
  onLink(listener: (error: string | undefined) => void): void {
    linkListeners.add(listener);
  },
  /** The language and theme of the person at this window. They are always read on this device. */
  async getViewer(): Promise<Viewer> {
    if (native) return call("getViewer");
    const { language, theme } = loadPreviewPreferences();
    return { language, theme };
  },
  /** Who answers, what this device may do there, and the version. */
  async getSession(): Promise<Session> {
    if (native) return call("getSession");
    previewGate("getSession");
    const device = previewDevices().devices.find(item => item.id === remoteId);
    return {
      device: { id: remoteId ?? "", name: device?.name ?? "Device", kind: device?.kind ?? "desktop" },
      permissions: previewPermissions(),
      version: previewQuery.has("mismatch") ? "0.5.0" : previewVersion,
      protocol: previewQuery.has("protocol") ? PROTOCOL + 1 : PROTOCOL,
    };
  },
  /** The name of the device before its session answers: the address of the window in the app, and the sample name in a preview. */
  async remoteName(): Promise<string> {
    // The app puts the name in the address of the window, as text, and this window may not read its own title.
    if (native) return (previewQuery.get("name") ?? "").slice(0, 80);
    return previewDevices().devices.find(item => item.id === remoteId)?.name ?? "Device";
  },
  /** Opens the window that controls a paired device, or brings it forward. */
  async openRemoteWindow(id: string): Promise<void> {
    if (native) return call("openRemoteWindow", { id });
    window.open(`${location.pathname}?remote=${encodeURIComponent(id)}`, "_blank");
  },
  /** The tools of another device, read for the Devices view. */
  async remoteStatuses(id: string): Promise<Statuses> {
    if (native) return call("remoteCall", { id, request: { command: "getStatuses" } });
    if (!previewDevices().devices.some(device => device.id === id)) throw "notPaired";
    return previewRemoteStatuses(id);
  },
  async remoteSession(id: string): Promise<Session> {
    if (native) return call("remoteCall", { id, request: { command: "getSession" } });
    return { ...(await desktop.getSession()), device: { id, name: "", kind: "desktop" } };
  },
  /** Turns a tool of another device on or off. Starting the proxy server is for the person at that device only. */
  async remoteSetToolActive(id: string, tool: ToolId, active: boolean): Promise<Statuses> {
    if (native) return call("remoteCall", { id, request: { command: "setToolActive", args: { tool, active } } });
    if (refusal("setToolActive", { tool, active }, undefined)) throw "localOnly";
    const running = previewRemoteStatuses(id) && previewRemoteRunning.get(id)!;
    if (active) running.add(tool);
    else running.delete(tool);
    return previewRemoteStatuses(id);
  },
  /** The installed applications of another device, and which are open. The device decides who may ask. */
  async remoteApplications(id: string): Promise<RemoteApp[]> {
    if (native) return call("remoteCall", { id, request: { command: "listApplications" } });
    if (!previewDevices().devices.some(device => device.id === id)) throw "notPaired";
    if (refusal("listApplications", undefined, previewPermissions())) throw "notPermitted";
    return previewApps.map(({ bundleId, name, version }) => ({ bundleId, name, version, open: previewOpen.has(bundleId) }));
  },
  /** Opens an application of another device. The device checks that the identifier is one that it listed. */
  async remoteOpenApplication(id: string, bundleId: string): Promise<void> {
    if (native) return call("remoteCall", { id, request: { command: "openApplication", args: { bundleId } } });
    if (refusal("openApplication", undefined, previewPermissions())) throw "notPermitted";
    await new Promise(resolve => setTimeout(resolve, 700));
    if (!previewApps.some(app => app.bundleId === bundleId)) throw "appNotFound";
    previewOpen.add(bundleId);
  },
  /** The address of the cached icon of an application of another device, or `null` when it has none. */
  async remoteAppIcon(id: string, bundleId: string, version: string): Promise<string | null> {
    if (native) {
      const { path } = await call<{ path: string }>("getCachedIcon", { id, bundleId, version });
      return convertFileSrc(path);
    }
    await new Promise(resolve => setTimeout(resolve, 150));
    return previewApps.find(app => app.bundleId === bundleId)?.icon ?? null;
  },
  /** Saves up to three applications that the Devices view shows first for a device. */
  async setPinnedApps(id: string, bundleIds: string[]): Promise<void> {
    if (native) return call("setPinnedApps", { id, bundleIds });
    previewPinned.splice(0, previewPinned.length, ...bundleIds);
  },
  macApp,
  /** True when the Devices view shows sample data, because no native app answers. */
  sampleDevices: !native,
  showWindowTitlebar: !native || macApp,
  showUpdateCheck: !native || macApp,
  showPresentationPrivacy: !native || macApp,
  async getVersion(): Promise<string> {
    return native ? getVersion() : previewVersion;
  },
  async getPreferences(): Promise<Preferences> {
    if (native) return call("getPreferences");
    // A preview shows What’s new with `?whatsnew`.
    return { ...loadPreviewPreferences(), seenVersion: previewQuery.has("whatsnew") ? null : previewVersion };
  },
  async getSystemLanguage(): Promise<Language> {
    // WebKit can expose only the primary preferred language to the webview.
    const local = native ? await call<Language | null>("getSystemLanguage").catch(() => null) : null;
    if (local) return local;
    for (const tag of navigator.languages) {
      const code = tag.toLowerCase().split(/[-_]/, 1)[0];
      if (code === "es" || code === "en") return code;
    }
    return "en";
  },
  async saveLanguage(language: Preferences["language"]): Promise<void> {
    if (native) return call("setLanguage", { language });
    savePreview(current => ({ ...current, language }));
  },
  async saveTheme(theme: Theme): Promise<void> {
    if (native) return call("setTheme", { theme });
    savePreview(current => ({ ...current, theme }));
  },
  async saveDuration(duration: Duration): Promise<void> {
    if (native) return call("setDuration", { duration });
    savePreview(current => ({ ...current, duration }));
  },
  async saveSchedule(schedule: Preferences["schedule"]): Promise<void> {
    if (native) return call("setSchedule", { schedule });
    savePreview(current => ({ ...current, schedule }));
  },
  async saveQuickStart(tool: ToolId, include: boolean): Promise<void> {
    if (native) return call("setQuickStart", { tool, include });
    savePreview(current => ({ ...current, quickStart: { ...current.quickStart, [tool]: include } }));
  },
  async saveStartToolsOnLaunch(enabled: boolean): Promise<void> {
    if (native) return call("setStartToolsOnLaunch", { enabled });
    savePreview(current => ({ ...current, startToolsOnLaunch: enabled }));
  },
  async saveHideWhileSharing(enabled: boolean): Promise<void> {
    if (native) return call("setHideWhileSharing", { enabled });
    savePreview(current => ({ ...current, hideWhileSharing: enabled }));
  },
  async saveSeenVersion(version: string | null): Promise<void> {
    if (native) return call("setSeenVersion", { version });
    savePreview(current => ({ ...current, seenVersion: version }));
  },
  async saveProxyRole(role: ProxySettings["role"]): Promise<void> {
    if (native) return call("setProxyRole", { role });
    previewGate("setProxyRole");
    savePreview(current => ({ ...current, proxy: { ...current.proxy, role } }));
  },
  async saveProxyPort(port: number): Promise<void> {
    if (native) return call("setProxyPort", { port });
    previewGate("setProxyPort");
    savePreview(current => ({ ...current, proxy: { ...current.proxy, port } }));
  },
  async saveProxyServer(server: string): Promise<void> {
    if (native) return call("setProxyServer", { server });
    previewGate("setProxyServer");
    savePreview(current => ({ ...current, proxy: { ...current.proxy, server } }));
  },
  /** The Access list is saved on this Mac only. */
  async saveProxyAllowed(allowed: string[]): Promise<void> {
    if (native) return call("saveProxyAllowed", { allowed });
    previewGate("saveProxyAllowed");
    savePreview(current => ({ ...current, proxy: { ...current.proxy, allowed } }));
  },
  /** The client list holds commands to run, so it is saved on this Mac only. */
  async saveProxyApps(apps: ProxyApp[]): Promise<void> {
    if (native) return call("saveProxyApps", { apps });
    previewGate("saveProxyApps");
    savePreview(current => ({ ...current, proxy: { ...current.proxy, apps } }));
  },
  async getLoginLaunch(): Promise<boolean> {
    return call("getLoginLaunch");
  },
  async setLoginLaunch(enabled: boolean): Promise<void> {
    return call("setLoginLaunch", { enabled });
  },
  startPreviewTools(preferences: Preferences): void {
    if (native || !preferences.startToolsOnLaunch) return;
    for (const id of ["activity", "caffeinate", "proxy"] as const) {
      if (preferences.quickStart[id]) setPreviewActive(id, true, preferences);
    }
  },
  async saveCaffeinateSettings(settings: CaffeinateSettings): Promise<Statuses> {
    if (native) return call("saveCaffeinateSettings", { settings });
    const error = previewCaffeinateError(settings);
    if (error) throw error;
    savePreviewPreferences({ ...loadPreviewPreferences(), caffeinate: settings });
    updatePreviewCaffeinateSession(settings);
    return previewStatuses();
  },
  async listProcesses(): Promise<RunningProcess[]> {
    return native ? call("listCaffeinateProcesses") : [
      { pid: 410, name: "Finder" },
      { pid: 625, name: "Safari" },
      { pid: 867, name: "Terminal" },
    ];
  },
  async getStatuses(): Promise<Statuses> {
    if (native) return call("getStatuses");
    previewGate("getStatuses");
    return previewStatuses();
  },
  async getProxyNetwork(): Promise<ProxyNetwork> {
    if (native) return call("getProxyNetwork");
    return { hostname: "studio.local", addresses: ["192.168.1.20"] };
  },
  async getProxyPeers(): Promise<ProxyPeer[]> {
    if (native) return call("getProxyPeers");
    const session = previewSessions.get("proxy");
    if (!session) return [];
    const seconds = Math.floor((performance.now() - session.started) / 1000);
    return [
      { address: "192.168.1.33", name: "MacBook-work", connections: 2 + seconds % 3, idleSeconds: seconds % 4 },
      { address: "192.168.1.50", name: null, connections: 0, idleSeconds: 720 + seconds },
    ];
  },
  async getProxyLog(): Promise<ProxyLogEntry[]> {
    if (native) return call("getProxyLog");
    const session = previewSessions.get("proxy");
    if (!session) return [];
    const targets = ["api.anthropic.com:443", "statsig.anthropic.com:443", "api.openai.com:443", "localhost:8080"];
    const count = Math.min(200, Math.floor((performance.now() - session.started) / 2500) + 1);
    const now = Math.floor(Date.now() / 1000);
    return Array.from({ length: count }, (_, index): ProxyLogEntry => {
      const id = count - 1 - index;
      const target = targets[id % targets.length];
      return {
        id,
        time: now - index * 3,
        peer: id % 7 === 6 ? "192.168.1.77" : "192.168.1.33",
        target: id % 7 === 6 ? "" : target,
        outcome: id % 7 === 6 ? "denied" : target.startsWith("localhost") ? "blocked" : "ok",
      };
    });
  },
  async getDevices(): Promise<DevicesOverview> {
    return native ? call("getDevices") : previewDevices();
  },
  async setPeerPermissions(
    id: string, permissions: Pick<Controller["permissions"], "control" | "applications" | "actions">,
  ): Promise<void> {
    if (native) return call("setPeerPermissions", { id, ...permissions });
    const controller = previewControllerList().find(item => item.id === id);
    if (!controller) throw "notPaired";
    controller.permissions = { ...controller.permissions, ...permissions };
  },
  /** The devices on this network that announce an SSH server, other than this one. */
  async discoverDevices(): Promise<DiscoveredDevice[]> {
    if (native) return call("discoverDevices");
    await new Promise(resolve => setTimeout(resolve, 1400));
    return [
      { name: "iMac del estudio", host: "imac-del-estudio.local", port: 22 },
      { name: "Mac mini de la sala", host: "mac-mini-de-la-sala.local", port: 22 },
    ];
  },
  /** Opens a window of ten minutes and returns the code to show. The permissions are those this device grants. */
  async openPairing(permissions: PairingPermissions): Promise<OpenedPairing> {
    if (native) return call("openPairing", { ...permissions });
    const expires = Math.floor(Date.now() / 1000) + 600;
    previewPairingOpened = Math.floor(Date.now() / 1000);
    Object.assign(previewPairing, { open: true, expires, attemptsLeft: 3 });
    return { code: "482913", account: "alex", expires };
  },
  async closePairing(): Promise<void> {
    if (native) return call("closePairing");
    Object.assign(previewPairing, { open: false, expires: null, attemptsLeft: 0 });
  },
  /** Pairs with the device at the address, using the code it shows. It can take up to a minute. */
  async pairDevice(user: string, host: string, port: number | null, code: string): Promise<RemoteDevice> {
    if (native) return call("pairDevice", { user, host, port, code });
    await new Promise(resolve => setTimeout(resolve, 900));
    if (!/^\d{6}$/.test(code)) throw "pairingCodeInvalid";
    // A preview takes one code, so a wrong one can be tried too.
    if (code !== "123456") throw "wrongCode";
    const device: RemoteDevice = {
      id: Array.from({ length: 32 }, () => Math.floor(Math.random() * 16).toString(16)).join(""),
      name: host, kind: "desktop", user, host, port: port ?? 22, added: Math.floor(Date.now() / 1000), online: true,
      tools: [], apps: { pinned: [] }, actions: [], shell: null,
    };
    previewDeviceAdditions.push(device);
    return device;
  },
  /** `removeOnDevice` is true: this device cannot take its own key out of the other one. */
  async forgetDevice(id: string): Promise<{ removeOnDevice: boolean }> {
    if (native) return call("forgetDevice", { id });
    const index = previewDeviceAdditions.findIndex(device => device.id === id);
    if (index >= 0) previewDeviceAdditions.splice(index, 1);
    return { removeOnDevice: true };
  },
  async removePeer(id: string): Promise<void> {
    if (native) return call("removePeer", { id });
    const controller = previewControllerList().find(item => item.id === id);
    if (!controller) throw "notPaired";
    previewControllers = previewControllerList().filter(item => item.id !== id);
    previewLogList().unshift({
      time: Math.floor(Date.now() / 1000), peer: id, name: controller.name, request: "removePeer", outcome: "ok",
    });
  },
  /** One administrator prompt turns on Remote Login, allows this user, and lets only keys in. */
  async enableRemoteAccess(): Promise<RemoteAccess> {
    if (native) return call("enableRemoteAccess");
    await new Promise(resolve => setTimeout(resolve, 900));
    if (previewQuery.has("remote-fails") && previewAccess.remoteLogin === "off") throw "remoteLoginStillOff";
    Object.assign(previewAccess, { remoteLogin: "on", passwordLogin: "blocked", userAllowed: true, managedFile: true });
    return { ...previewAccess };
  },
  async allowPasswordLogin(): Promise<RemoteAccess> {
    if (native) return call("allowPasswordLogin");
    Object.assign(previewAccess, { passwordLogin: "allowed", managedFile: false });
    return { ...previewAccess };
  },
  async openSharingSettings(): Promise<void> {
    if (native) return call("openSharingSettings");
    // A preview acts as if the user turned Remote Login on and allowed themselves there.
    Object.assign(previewAccess, { remoteLogin: "on", userAllowed: true, passwordLogin: previewAccess.passwordLogin === "unknown" ? "allowed" : previewAccess.passwordLogin });
  },
  async cutAllAccess(): Promise<void> {
    if (native) return call("cutAllAccess");
    previewControllers = [];
    previewLogList().unshift({
      time: Math.floor(Date.now() / 1000), peer: null, name: null, request: "cutAll", outcome: "ok",
    });
  },
  /** The entries of the log, newest first. */
  async getRemoteLog(): Promise<RemoteLogEntry[]> {
    return native ? call("getRemoteLog") : previewLogList().map(entry => ({ ...entry }));
  },
  async detectProxyApps(): Promise<ProxyInstalls> {
    return native ? call("detectProxyApps") : { claudeCode: true, claudeApp: true, codexCode: false, codexApp: true };
  },
  async checkProxyServer(address: string): Promise<boolean> {
    return native ? call("checkProxyServer", { address }) : !address.startsWith("offline");
  },
  async launchProxyApp(id: string): Promise<void> {
    if (native) return call("launchProxyApp", { id });
  },
  async getActivityAccessibility(): Promise<ActivityAccessibility> {
    // A preview shows a missing permission with `?accessibility=required`.
    return native ? call("getActivityAccessibility") : (previewQuery.get("accessibility") as ActivityAccessibility | null) ?? "unsupported";
  },
  async requestActivityAccessibility(): Promise<void> {
    if (native) return call("requestActivityAccessibility");
    previewGate("requestActivityAccessibility");
  },
  /** True on the first launch of a new installation. A preview shows the setup with `?onboarding`. */
  async getFirstRun(): Promise<boolean> {
    return native ? call("getFirstRun") : previewQuery.has("onboarding");
  },
  async getLidStatus(): Promise<LidStatus> {
    // A preview shows a Mac that already stays awake with `?lid-external`.
    return native ? call("getLidStatus") : { supported: true, external: previewQuery.has("lid-external") };
  },
  /** Saves the choice. macOS can ask for an administrator password before it answers. */
  async setLidMode(mode: LidMode): Promise<LidStatus> {
    if (native) return call("setLidMode", { mode });
    await new Promise(resolve => setTimeout(resolve, 500));
    return { supported: true, external: false };
  },
  async getUpdateStatus(): Promise<UpdateStatus> {
    return macApp ? call("getUpdateStatus") : previewUpdate;
  },
  async checkForUpdate(): Promise<UpdateStatus> {
    if (macApp) return call("checkForUpdate");
    await new Promise(resolve => setTimeout(resolve, 700));
    // A preview finds an update with `?update=<version>`.
    previewUpdate = { version: previewQuery.get("update"), checked: Math.floor(Date.now() / 1000) };
    return previewUpdate;
  },
  async onAvailableUpdate(refresh: () => void): Promise<void> {
    if (macApp) await listen("available-update-changed", refresh);
  },
  /** Installs the latest release, which can be newer than the one the notice named, and restarts. */
  async restartToUpdate(): Promise<void> {
    if (macApp) await call("restartToUpdate");
  },
  onFocus(refresh: () => void): void {
    if (native) {
      void getCurrentWindow().onFocusChanged(({ payload: focused }) => {
        if (focused) refresh();
      }).catch(() => window.addEventListener("focus", refresh));
    } else {
      window.addEventListener("focus", refresh);
    }
  },
  async setToolActive(tool: ToolId, active: boolean): Promise<Statuses> {
    if (native) return call("setToolActive", { tool, active });
    previewGate("setToolActive", { tool, active });
    setPreviewActive(tool, active, loadPreviewPreferences());
    return previewStatuses();
  },
  setTheme(theme: Theme): void {
    if (native) void getCurrentWindow().setTheme(theme === "system" ? null : theme).catch(() => {});
  },
};
