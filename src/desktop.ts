import { getVersion } from "@tauri-apps/api/app";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { version as previewVersion } from "../src-tauri/tauri.conf.json";
import type { Language } from "./messages";
import type { ToolId } from "./modules";

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

export const defaults: Preferences = {
  language: "system",
  theme: "system",
  duration: "minutes15",
  schedule: { enabled: false, start: "09:00", end: "17:00" },
  quickStart: { activity: false, caffeinate: false, proxy: false },
  startToolsOnLaunch: false,
  hideWhileSharing: true,
  caffeinate: {
    mode: "untilStopped",
    flags: { i: true, d: false, m: false, s: false, u: false },
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

export const desktop = {
  macApp,
  showWindowTitlebar: !native || macApp,
  showUpdateCheck: !native || macApp,
  showPresentationPrivacy: !native || macApp,
  async getVersion(): Promise<string> {
    return native ? getVersion() : previewVersion;
  },
  async getPreferences(): Promise<Preferences> {
    return native ? invoke("get_preferences") : loadPreviewPreferences();
  },
  async getSystemLanguage(): Promise<Language> {
    // WebKit can expose only the primary preferred language to the webview.
    const local = native ? await invoke<Language | null>("get_system_language").catch(() => null) : null;
    if (local) return local;
    for (const tag of navigator.languages) {
      const code = tag.toLowerCase().split(/[-_]/, 1)[0];
      if (code === "es" || code === "en") return code;
    }
    return "en";
  },
  async savePreferences(preferences: Preferences): Promise<void> {
    if (native) return invoke("save_preferences", { preferences });
    savePreviewPreferences({ ...preferences, caffeinate: loadPreviewPreferences().caffeinate });
  },
  async getLoginLaunch(): Promise<boolean> {
    return invoke("get_login_launch");
  },
  async setLoginLaunch(enabled: boolean): Promise<void> {
    return invoke("set_login_launch", { enabled });
  },
  startPreviewTools(preferences: Preferences): void {
    if (native || !preferences.startToolsOnLaunch) return;
    for (const id of ["activity", "caffeinate", "proxy"] as const) {
      if (preferences.quickStart[id]) setPreviewActive(id, true, preferences);
    }
  },
  async saveCaffeinateSettings(settings: CaffeinateSettings): Promise<Statuses> {
    if (native) return invoke("save_caffeinate_settings", { settings });
    const error = previewCaffeinateError(settings);
    if (error) throw error;
    savePreviewPreferences({ ...loadPreviewPreferences(), caffeinate: settings });
    updatePreviewCaffeinateSession(settings);
    return previewStatuses();
  },
  async listProcesses(): Promise<RunningProcess[]> {
    return native ? invoke("list_caffeinate_processes") : [
      { pid: 410, name: "Finder" },
      { pid: 625, name: "Safari" },
      { pid: 867, name: "Terminal" },
    ];
  },
  async getStatuses(): Promise<Statuses> {
    return native ? invoke("get_statuses") : previewStatuses();
  },
  async getProxyNetwork(): Promise<ProxyNetwork> {
    if (native) return invoke("get_proxy_network");
    return { hostname: "studio.local", addresses: ["192.168.1.20"] };
  },
  async getProxyPeers(): Promise<ProxyPeer[]> {
    if (native) return invoke("get_proxy_peers");
    const session = previewSessions.get("proxy");
    if (!session) return [];
    const seconds = Math.floor((performance.now() - session.started) / 1000);
    return [
      { address: "192.168.1.33", name: "MacBook-work", connections: 2 + seconds % 3, idleSeconds: seconds % 4 },
      { address: "192.168.1.50", name: null, connections: 0, idleSeconds: 720 + seconds },
    ];
  },
  async getProxyLog(): Promise<ProxyLogEntry[]> {
    if (native) return invoke("get_proxy_log");
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
  async detectProxyApps(): Promise<ProxyInstalls> {
    return native ? invoke("detect_proxy_apps") : { claudeCode: true, claudeApp: true, codexCode: false, codexApp: true };
  },
  async checkProxyServer(address: string): Promise<boolean> {
    return native ? invoke("check_proxy_server", { address }) : !address.startsWith("offline");
  },
  async launchProxyApp(id: string): Promise<void> {
    if (native) return invoke("launch_proxy_app", { id });
  },
  async getActivityAccessibility(): Promise<ActivityAccessibility> {
    return native ? invoke("get_activity_accessibility") : "unsupported";
  },
  async requestActivityAccessibility(): Promise<void> {
    if (native) return invoke("request_activity_accessibility");
  },
  async getUpdateStatus(): Promise<UpdateStatus> {
    return macApp ? invoke("get_update_status") : previewUpdate;
  },
  async checkForUpdate(): Promise<UpdateStatus> {
    if (macApp) return invoke("check_for_update");
    await new Promise(resolve => setTimeout(resolve, 700));
    previewUpdate = { version: null, checked: Math.floor(Date.now() / 1000) };
    return previewUpdate;
  },
  async onAvailableUpdate(refresh: () => void): Promise<void> {
    if (macApp) await listen("available-update-changed", refresh);
  },
  async restartToUpdate(version: string): Promise<void> {
    if (macApp) await invoke("restart_to_update", { version });
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
    if (native) return invoke("set_tool_active", { tool, active });
    setPreviewActive(tool, active, loadPreviewPreferences());
    return previewStatuses();
  },
  setTheme(theme: Theme): void {
    if (native) void getCurrentWindow().setTheme(theme === "system" ? null : theme).catch(() => {});
  },
};
