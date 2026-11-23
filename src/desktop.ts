import { getVersion } from "@tauri-apps/api/app";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { version as previewVersion } from "../src-tauri/tauri.conf.json";
import type { Language } from "./messages";
import type { ToolId } from "./modules";
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
  permissions: Record<ControllerPermission, boolean>;
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
/** `icon` is the address of an image file, or `null` for the generic icon. */
export type RemoteApp = { id: string; name: string; icon: string | null; open: boolean };
export type RemoteDevice = {
  id: string;
  name: string;
  kind: DeviceKind;
  online: boolean;
  tools: RemoteTool[];
  /** `pinned` holds the apps the user chose to see first. `total` and `open` count every app on the device. */
  apps: { pinned: RemoteApp[]; total: number; open: number };
  actions: SavedAction[];
  /** The shell is on until this time, or `null` while it is off. Only that device can turn it on. */
  shell: { until: number } | null;
};
export type DevicesOverview = {
  thisDevice: {
    /** Empty when the system gives no name. */
    name: string;
    kind: DeviceKind;
    remoteAccess: "unknown" | "off" | "on";
    pairing: "closed" | "open";
    controllers: Controller[];
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
  return invoke<T>("dispatch", { request: { command, args } });
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

/** The sample list a preview shows, so the whole Devices view can be checked. Times follow the clock. */
function previewDevices(): DevicesOverview {
  const now = Math.floor(Date.now() / 1000);
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
      name: "MacBook Pro", kind: "laptop", remoteAccess: "on", pairing: "closed",
      controllers: [{
        id: "studio", name: "iMac del estudio", kind: "desktop", lastAccess: now - 5 * 60,
        permissions: { control: true, applications: true, actions: true, shell: false },
      }],
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
        tools: [{ id: "activity", running: true }, { id: "caffeinate", running: false }],
        apps: {
          total: 27, open: 2,
          pinned: [
            { id: "safari", name: "Safari", icon: safariIcon, open: true },
            { id: "notes", name: "Notes", icon: notesIcon, open: false },
            { id: "claude", name: "Claude", icon: claudeIcon, open: true },
          ],
        },
        actions: [backup, docker],
        shell: { until: Math.floor(shellEnd.getTime() / 1000) },
      },
      {
        id: "old", name: "MacBook Air de la oficina con un nombre muy largo para probar el ajuste", kind: "laptop",
        online: false, tools: [], apps: { pinned: [], total: 0, open: 0 }, actions: [], shell: null,
      },
    ],
  };
}

export const desktop = {
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
    savePreview(current => ({ ...current, proxy: { ...current.proxy, role } }));
  },
  async saveProxyPort(port: number): Promise<void> {
    if (native) return call("setProxyPort", { port });
    savePreview(current => ({ ...current, proxy: { ...current.proxy, port } }));
  },
  async saveProxyServer(server: string): Promise<void> {
    if (native) return call("setProxyServer", { server });
    savePreview(current => ({ ...current, proxy: { ...current.proxy, server } }));
  },
  /** The Access list is saved on this Mac only. */
  async saveProxyAllowed(allowed: string[]): Promise<void> {
    if (native) return call("saveProxyAllowed", { allowed });
    savePreview(current => ({ ...current, proxy: { ...current.proxy, allowed } }));
  },
  /** The client list holds commands to run, so it is saved on this Mac only. */
  async saveProxyApps(apps: ProxyApp[]): Promise<void> {
    if (native) return call("saveProxyApps", { apps });
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
    return native ? call("getStatuses") : previewStatuses();
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
    setPreviewActive(tool, active, loadPreviewPreferences());
    return previewStatuses();
  },
  setTheme(theme: Theme): void {
    if (native) void getCurrentWindow().setTheme(theme === "system" ? null : theme).catch(() => {});
  },
};
