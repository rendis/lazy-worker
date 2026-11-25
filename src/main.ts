import "./style.css";
import {
  Activity, AppWindow, ArrowLeft, ArrowRight, Bell, BellOff, Bot, Box, Brain, Calendar, CalendarClock, CalendarDays, Check, ChevronDown, ChevronRight, CircleCheck, CircleHelp, CircleMinus, CircleStop, CircleX, Clock, Cloud, Code, Coffee, Contrast, Copy,
  Cpu, createIcons, Database, Download, ExternalLink, Eye, EyeOff, File as FileIcon, FileArchive, FileImage, FileText, Folder, FolderKey, FolderOpen, FolderPlus, FolderUp, Globe, HardDrive, Hand, Hash, Hourglass, House, Image, Inbox, Infinity as InfinityIcon, Info, KeyRound, Laptop, LaptopMinimal, LayoutGrid, Link, List, LoaderCircle, LockKeyhole, LockKeyholeOpen,
  Mail, Maximize2, Megaphone, MessageSquare, Monitor, MonitorSmartphone, MonitorUp, Moon, MousePointer2, Network, Paperclip, Pencil, Pin, PinOff, Play, PlugZap, Plus, Power, Radio, RefreshCw, Repeat, Rocket, RotateCcw, ScrollText, Search, Send, Server, Settings,
  Shield, ShieldCheck, ShieldOff, Sparkles, Square, SquareArrowOutUpRight, SquareTerminal, Sun, Sunrise, Sunset, Tag, Terminal, Timer, Trash2, TriangleAlert, Upload, UserMinus, UserRound, Volume2, X, Zap,
} from "lucide";
import { BELL_AGE_WIDTH, mountBell } from "./bell";
import { mountCaffeinateForm } from "./caffeinate-form";
import { mountDevices } from "./devices";
import { mountMailbox } from "./mailbox";
import {
  defaults, desktop, offStatus, type ActivityAccessibility, type LidMode, type LidStatus, type CaffeinateSettings, type Duration, type Preferences,
  type NotificationMode, type NotificationPermission, type ProxySettings, type SoundName, type Statuses, type Theme,
} from "./desktop";
import { messages, type Language, type Message } from "./messages";
import { mountTools, tools, updateQuickStartMode, updateToolStatus, type ToolId } from "./modules";
import { mountOperationProgress } from "./operation-progress";
import { mountChangelog, releasesSince } from "./changelog";
import { mountOnboarding } from "./onboarding";
import { mountProxy } from "./proxy";
import { mountRemoteBanner } from "./remote-banner";
import { mountTabs } from "./tabs";
import { isLinkError, refusal, type Reason, type Session } from "./remote";
import { formatSession, SESSION_WIDTH } from "./time";
import { mountUpdateNotice } from "./update-notice";

function element<T extends HTMLElement = HTMLElement>(id: string): T {
  return document.getElementById(id) as T;
}

element("window-titlebar").hidden = !desktop.showWindowTitlebar;
/** The device that this window controls, or `null` in the panel. Rust decides it by the window label, and this only shapes the interface. */
const remote = desktop.remote;
let remoteName = "";
let session: Session | undefined;
if (remote) {
  document.querySelector<HTMLElement>(".workspace")!.dataset.remote = "";
  for (const node of document.querySelectorAll<HTMLElement>("[data-local-only]")) node.hidden = true;
}
document.documentElement.style.setProperty("--session-width", `${SESSION_WIDTH}ch`);
document.documentElement.style.setProperty("--bell-age-width", `${BELL_AGE_WIDTH}ch`);
const updateToolOrder = mountTools(pinned => updatePreference("tools", {
  key: "pinned",
  apply: current => ({ ...current, pinned }),
}));
function renderIcons(): void {
  createIcons({
    icons: {
      Activity, AppWindow, ArrowLeft, ArrowRight, Bell, BellOff, Bot, Box, Brain, Calendar, CalendarClock, CalendarDays, Check, ChevronDown, ChevronRight, CircleCheck, CircleHelp, CircleMinus, CircleStop, CircleX, Clock, Cloud, Code, Coffee, Contrast, Copy,
      Cpu, Database, Download, ExternalLink, Eye, EyeOff, File: FileIcon, FileArchive, FileImage, FileText, Folder, FolderKey, FolderOpen, FolderPlus, FolderUp, Globe, HardDrive, Hand, Hash, Hourglass, House, Image, Inbox, Infinity: InfinityIcon, Info, KeyRound, Laptop, LaptopMinimal, LayoutGrid, Link, List, LoaderCircle, LockKeyhole, LockKeyholeOpen,
      Mail, Maximize2, Megaphone, MessageSquare, Monitor, MonitorSmartphone, MonitorUp, Moon, MousePointer2, Network, Paperclip, Pencil, Pin, PinOff, Play, PlugZap, Plus, Power, Radio, RefreshCw, Repeat, Rocket, RotateCcw, ScrollText, Search, Send, Server,
      Settings, Shield, ShieldCheck, ShieldOff, Sparkles, Square, SquareArrowOutUpRight, SquareTerminal, Sun, Sunrise, Sunset, Tag, Terminal, Timer, Trash2, TriangleAlert, Upload, UserMinus, UserRound, Volume2, X, Zap,
    },
    attrs: { "aria-hidden": "true", "stroke-width": 1.75 },
  });
}
renderIcons();

const caffeinate = mountCaffeinateForm();
const progress = mountOperationProgress(element("operation-progress"));
const banner = mountRemoteBanner({ translate: key => translate(key), language: preferredLanguage });
if (remote) {
  let wasLost = false;
  desktop.onLink(error => {
    banner.setLink(error);
    // The link came back, possibly after the other device restarted. Its version and settings may have changed.
    if (error === undefined && wasLost) void reloadRemote();
    wasLost = error !== undefined;
  });
}
const activityModes = element("activity-modes");
const modeChoices = activityModes.querySelectorAll<HTMLInputElement>('input[name="activity-mode"]');
const activityDuration = element("activity-duration");
const activityPower = element<HTMLButtonElement>("power");
const scheduleForm = element<HTMLFormElement>("schedule-form");
const scheduleSave = element<HTMLButtonElement>("schedule-save");
const scheduleSaveLabel = element("schedule-save-label");
const scheduleDisable = element<HTMLButtonElement>("schedule-disable");
const scheduleState = element("schedule-state");
const start = element<HTMLInputElement>("start");
const end = element<HTMLInputElement>("end");
const startSelected = element<HTMLButtonElement>("start-selected");
const stopSelected = element<HTMLButtonElement>("stop-selected");
const sessionTimers = document.querySelectorAll<HTMLElement>("[data-tool-timer]");
const statusLabels = document.querySelectorAll<HTMLElement>("[data-tool-status]");
const toolNavButtons = document.querySelectorAll<HTMLButtonElement>(".tool-nav-button");
const powerButtons = document.querySelectorAll<HTMLButtonElement>("[data-tool-power]");
const includeSwitches = document.querySelectorAll<HTMLInputElement>("[data-tool-include]");
const quickStartHints = document.querySelectorAll<HTMLElement>("[data-quick-start-hint]");
const durationButtons = document.querySelectorAll<HTMLButtonElement>("[data-duration]");
const languageButtons = document.querySelectorAll<HTMLButtonElement>("[data-language]");
const themeButtons = document.querySelectorAll<HTMLButtonElement>("[data-theme-choice]");
const notificationModeButtons = document.querySelectorAll<HTMLButtonElement>("[data-notification-mode]");
const soundNameButtons = document.querySelectorAll<HTMLButtonElement>("[data-sound-name]");
const notificationSound = element<HTMLInputElement>("notification-sound");
const notificationSoundSection = element("notification-sound-section");
const notificationPermission = element("notification-permission");
const notificationPermissionBox = element("notification-permission-box");
const notificationPermissionText = element("notification-permission-text");
const notificationPermissionAction = element<HTMLButtonElement>("notification-permission-action");
/** Whether macOS lets the app show banners, as last read. */
let notificationPermissionState: NotificationPermission = "unknown";
const notificationSoundPlay = element<HTMLButtonElement>("notification-sound-play");
const translated = document.querySelectorAll<HTMLElement>("[data-text]");
const accessibleLabels = document.querySelectorAll<HTMLElement>("[data-label]");
const translatedTips = document.querySelectorAll<HTMLElement>("[data-tip-text]");
// A tip inside a label must not switch the option of that label.
translatedTips.forEach(node => {
  if (node.classList.contains("option-info")) node.addEventListener("click", event => event.preventDefault());
});
const main = document.querySelector("main")!;
const noticeRegion = element("notice-region");
const noticeTitle = element("notice-title");
const noticeDescription = element("notice-description");
const noticeAnnouncement = element("notice-announcement");
const noticeAccessibility = element<HTMLButtonElement>("notice-accessibility");
const noticeDismiss = element<HTMLButtonElement>("notice-dismiss");
const permissionRegion = element("permission-notice");
const permissionTitle = element("permission-notice-title");
const permissionAccessibility = element<HTMLButtonElement>("permission-notice-accessibility");
const permissionDismiss = element<HTMLButtonElement>("permission-notice-dismiss");
const accessibilityStatus = element("accessibility-status");
const settingsAccessibility = element<HTMLButtonElement>("settings-accessibility");
const settingsButton = element<HTMLButtonElement>("settings-button");
const settingsPreferences = element("settings-preferences");
const settingsPermissionDot = element("settings-permission-dot");
const settingsPermissionState = element("settings-permission-state");
const permissionFeatures = element("permission-features");
const loginLaunch = element<HTMLInputElement>("login-launch");
const startToolsOnLaunch = element<HTMLInputElement>("start-tools-on-launch");
const hideWhileSharing = element<HTMLInputElement>("hide-while-sharing");
const permissionWarnings = document.querySelectorAll<HTMLButtonElement>("[data-permission-warning]");
const activityWarning = document.querySelector<HTMLButtonElement>('[data-permission-warning="activity"]')!;
const sharingWarning = document.querySelector<HTMLButtonElement>('[data-permission-warning="hideWhileSharing"]')!;
element("settings-presentation").hidden = !desktop.showPresentationPrivacy;
element("login-launch-setting").hidden = !desktop.macApp;
const settingsTabKey = "lazy-worker-settings-tab";
/** The tabs of Settings. The last tab chosen shows again, and a tab whose content waits for the person carries a dot. */
const settingsTabs = mountTabs(element("settings-tabs"), [...document.querySelectorAll<HTMLElement>(".settings-panel")], {
  initial: (() => {
    try {
      return localStorage.getItem(settingsTabKey) ?? undefined;
    } catch {
      return undefined;
    }
  })(),
  onSelect: id => {
    try {
      localStorage.setItem(settingsTabKey, id);
    } catch {
      // Without storage Settings opens on its first tab next time.
    }
  },
});

type ActivityMode = "duration" | "schedule";
type FeedbackTarget = ToolId | "tools" | "settings" | "devices";
type FeedbackLine = { message: Message; tool?: ToolId };
type PreferenceChoice = "duration" | "language" | "theme" | "hideWhileSharing" | "startToolsOnLaunch" | "seenVersion" | "notifications";
type PreferenceChange = {
  key: PreferenceChoice | ToolId | "proxySettings" | "pinned";
  apply: (current: Preferences) => Preferences;
};
/** The notices that report no failure: a saved change, and a request that another device keeps for itself. */
const calmMessages = new Set<Message>([
  "saved", "forgetRemoveThere", "localOnly", "notPermitted", "lidNeedsLocalPermission", "remoteLost",
]);
const accessibilityLabels: Record<ActivityAccessibility, Message> = {
  granted: "accessibilityGranted",
  required: "accessibilityRequired",
  unsupported: "accessibilityUnsupported",
  unavailable: "accessibilityUnavailable",
};

let savedPreferences = defaults;
let preferences = savedPreferences;
let activityMode: ActivityMode = "duration";
const systemAppearance = window.matchMedia("(prefers-color-scheme: dark)");
let systemLanguage: Language = "en";
let renderedLanguage: Language | undefined;
const pendingPreferences: PreferenceChange[] = [];
let queue = Promise.resolve();
let statuses: Statuses = { activity: offStatus, caffeinate: offStatus, proxy: offStatus };
const startFailures: Record<ToolId, Message> = {
  activity: "workerUnavailable", caffeinate: "caffeinateStartFailed", proxy: "proxyStartFailed",
};
const unsupportedTools: Record<ToolId, Message> = {
  activity: "unsupported", caffeinate: "caffeinateUnsupported", proxy: "proxyUnsupported",
};
let workerAction: { target: FeedbackTarget; running: boolean; keepQuickStartLayout: boolean } | undefined;
let ready = false;
let loaded = false;
let statusRevision = 0;
let toolFailures: FeedbackLine[] = [];
let toolFailureTitle: Message = "selectedStartFailed";
const feedback: Partial<Record<FeedbackTarget, Message>> = {};
const dismissedFeedback: Partial<Record<FeedbackTarget, string>> = {};
let visibleNotice: { target: FeedbackTarget; key: string } | undefined;
let accessibility: ActivityAccessibility | undefined;
let accessibilityRefresh: Promise<void> | undefined;
let accessibilityOpening = false;
let loginLaunchPending = false;
let accessibilityRequestIssue: FeedbackTarget | undefined;
let permissionNotice: { trigger: HTMLButtonElement; pinned: boolean } | undefined;
let permissionNoticeDismissal: number | undefined;
let activityResultRevision = 0;
let activityPermissionErrorRetired = false;

let caffeinateSaveDelay: number | undefined;
let caffeinateEditRevision = 0;
let caffeinateSaving = false;

function preferredLanguage(): Language {
  return preferences.language === "system" ? systemLanguage : preferences.language;
}

function translate(key: Message): string {
  const text = messages[preferredLanguage()][key];
  // In a window that controls a device, `{name}` is that device. No other text has one there.
  return remote ? text.split("{name}").join(remoteName) : text;
}

function errorMessage(error: unknown, fallback: Message): Message {
  if (remote && isLinkError(error)) return "remoteLost";
  return typeof error === "string" && Object.prototype.hasOwnProperty.call(messages.en, error)
    ? error as Message : fallback;
}

function setText(node: HTMLElement, text: string): void {
  if (node.textContent !== text) node.textContent = text;
}

function setAttribute(node: HTMLElement, name: string, value: string): void {
  if (node.getAttribute(name) !== value) node.setAttribute(name, value);
}

function setDisabled(control: HTMLButtonElement | HTMLInputElement, disabled: boolean): void {
  if (control.disabled !== disabled) control.disabled = disabled;
}

function activeView(): HTMLElement {
  return document.querySelector<HTMLElement>("[data-module-view]:not([hidden])")!;
}

function toolError(tool: ToolId): Message | undefined {
  const { error, supported } = statuses[tool];
  if (error && !(tool === "activity" && error === "permissionRequired" && activityPermissionErrorRetired)) {
    return errorMessage(error, startFailures[tool]);
  }
  if (ready && !supported) return unsupportedTools[tool];
}

function needsAccessibility(): boolean {
  return desktop.macApp && (accessibility === "required" || accessibility === "unavailable");
}

/** Opens or closes a `.collapse`. A closed one takes no focus. */
function setCollapseOpen(region: HTMLElement, open: boolean): void {
  region.toggleAttribute("data-open", open);
  region.inert = !open;
}

function setNoticeOpen(region: HTMLElement, open: boolean): void {
  region.toggleAttribute("data-open", open);
  region.toggleAttribute("inert", !open);
  setAttribute(region, "aria-hidden", String(!open));
}

const onboarding = mountOnboarding({
  translate,
  accessibility: () => accessibility,
  openAccessibility: () => { void openAccessibility("settings"); },
  onClose: () => {
    markVersionSeen();
    // The update notice waits for the setup guide to close.
    updates.render();
  },
});
const caffeinateCopy = element<HTMLButtonElement>("caffeinate-copy");
let caffeinateCopied: number | undefined;
caffeinateCopy.addEventListener("click", () => {
  void desktop.copyText(element("caffeinate-preview").textContent ?? "").then(() => {
    // The check mark confirms the copy for a moment.
    caffeinateCopy.setAttribute("data-copied", "");
    window.clearTimeout(caffeinateCopied);
    caffeinateCopied = window.setTimeout(() => caffeinateCopy.removeAttribute("data-copied"), 1400);
  }).catch(() => {});
});
const lidModes = element("caffeinate-lid");
const lidChoices = [...lidModes.querySelectorAll<HTMLInputElement>("input")];
const lidCollapse = element("caffeinate-lid-collapse");
const lidNote = element("lid-note");
const lidNotes: Record<Exclude<LidMode, "sleep">, Message> = { charger: "caffeinateLidChargerNote", always: "caffeinateLidAlwaysNote" };
let lidStatus: LidStatus | undefined;
let lidPending: LidMode | undefined;
function renderLid(): void {
  // A Mac that already stays awake shows that choice, whatever is saved.
  // While macOS asks for the password, the row shows the requested choice.
  const shown = lidPending ?? (lidStatus?.external ? "always" : preferences.lidAwake);
  for (const choice of lidChoices) {
    choice.checked = choice.value === shown;
    setDisabled(choice, !loaded || lidPending !== undefined || !lidStatus?.supported);
  }
  lidModes.dataset.selection = shown;
  // The note keeps its last text while it closes.
  if (shown !== "sleep") setText(lidNote, translate(lidStatus?.external && !lidPending ? "caffeinateLidExternalNote" : lidNotes[shown]));
  element("lid-note-box").toggleAttribute("data-warning", shown === "always");
  lidCollapse.toggleAttribute("data-open", shown !== "sleep");
  lidCollapse.inert = shown === "sleep";
}
async function refreshLid(): Promise<void> {
  if (lidPending) return;
  lidStatus = await desktop.getLidStatus().catch(() => lidStatus);
  renderLid();
}
async function chooseLid(mode: LidMode): Promise<void> {
  lidPending = mode;
  clearFeedback("caffeinate");
  renderLid();
  try {
    lidStatus = await desktop.setLidMode(mode);
    preferences = { ...preferences, lidAwake: mode };
    savedPreferences = { ...savedPreferences, lidAwake: mode };
  } catch (error) {
    // A canceled password prompt leaves the choice as it was, with no error.
    if (error !== "lidAuthorizationCanceled") feedback.caffeinate = errorMessage(error, "lidAuthorizationFailed");
  } finally {
    lidPending = undefined;
    renderLid();
    renderFeedback();
  }
}
lidModes.addEventListener("input", event => {
  event.stopPropagation();
  void chooseLid((event.target as HTMLInputElement).value as LidMode);
});
const changelog = mountChangelog({ translate, language: preferredLanguage, onClose: markVersionSeen });
/** Opens a place of the panel that a notification names. Only the views of this panel are known. */
function openRoute(route: string): void {
  // `mailbox:device:<id>` or `mailbox:peer:<id>` opens that conversation in Devices.
  const box = /^mailbox:(device|peer):([0-9a-f]{32})$/.exec(route);
  if (box && !remote) {
    document.querySelector<HTMLButtonElement>('.sidebar [data-view="devices"]')?.click();
    void desktop.getDevices().then(overview => {
      const id = box[2];
      const named = box[1] === "device"
        ? overview.devices.find(device => device.id === id)
        : overview.thisDevice.controllers.find(controller => controller.id === id);
      if (!named) return;
      const online = box[1] === "device" ? (named as { online: boolean | null }).online === true : null;
      mailbox.open(box[1] === "device" ? { device: id } : { peer: id }, named.name, online);
    }).catch(() => {});
    return;
  }
  if (!/^[a-z]+$/.test(route)) return;
  // Only an update sends people to Settings, and its section is under About.
  if (route === "settings") settingsTabs.select("about");
  document.querySelector<HTMLButtonElement>(`.sidebar [data-view="${route}"]`)?.click();
}
const bell = remote ? undefined : mountBell({ translate, language: preferredLanguage, refreshIcons: renderIcons, navigate: openRoute });
if (bell) {
  void desktop.onNotificationOpen(route => {
    if (route === "bell") bell.open();
    else openRoute(route);
  });
}
/** The problem of each view that the bell already has, so a notice that stays open is listed once. */
const recordedFeedback: Partial<Record<FeedbackTarget, string>> = {};
element("changelog-open").addEventListener("click", event => changelog.open(undefined, event.currentTarget as HTMLElement));
let runningVersion: string | undefined;
/** Records that the user saw the setup or the changes of the running version. */
function markVersionSeen(): void {
  if (runningVersion && !remote) updateChoice("settings", "seenVersion", runningVersion);
}
const updates = mountUpdateNotice({
  translate,
  setNoticeOpen,
  beforeRestart: () => {
    flushCaffeinateAutosave();
    return queue;
  },
  moveFocus: () => activeView().querySelector("h1")?.focus({ preventScroll: true }),
  held: () => onboarding.isOpen(),
  showWaiting: waiting => settingsTabs.summary("about", waiting ? translate("updates") : "", waiting && "pending"),
});

function quickStartHint(): string {
  return translate(preferences.startToolsOnLaunch ? "quickStartOnLaunch" : "quickStartOnDemand");
}

const proxy = mountProxy({
  translate,
  quickStartHint,
  update: apply => updatePreference("proxy", {
    key: "proxySettings",
    apply: current => ({ ...current, proxy: apply(current.proxy) }),
  }),
  fail: message => {
    feedback.proxy = message;
    delete dismissedFeedback.proxy;
    renderFeedback();
  },
  refreshIcons: renderIcons,
  // The proxy's role, port, address list, and client commands are kept by the person at the device.
  lockTip: () => remote ? translate(refusal("setProxyRole", undefined, session?.permissions) ?? "localOnly") : undefined,
});

const devices = mountDevices({
  translate,
  language: preferredLanguage,
  refreshIcons: renderIcons,
  fail(error) {
    feedback.devices = errorMessage(error, "remoteChangeFailed");
    renderFeedback();
  },
  notify(message) {
    feedback.devices = message;
    renderFeedback();
  },
  openMailbox(party, name, online, trigger) {
    mailbox.open(party, name, online, trigger);
  },
});
/** The mailbox window. Only the panel has one: a window that controls a device has no mailbox. */
const mailbox = mountMailbox({ translate, language: preferredLanguage, refreshIcons: renderIcons, onChange: () => void devices.readMailboxes() });

function renderFeedback(): void {
  const unsupported = accessibility === "unsupported";
  const permissionPending = desktop.macApp && accessibility === "required";
  settingsPreferences.toggleAttribute("data-permission-pending", permissionPending);
  settingsPreferences.toggleAttribute("data-permission-granted", accessibility === "granted");
  settingsPermissionDot.hidden = !permissionPending;
  settingsTabs.summary("privacy", permissionPending ? translate("accessibilityPending") : "", permissionPending && "pending");
  settingsPermissionState.hidden = !permissionPending;
  permissionFeatures.hidden = !permissionPending;
  if (permissionPending) setAttribute(settingsButton, "aria-describedby", settingsPermissionState.id);
  else settingsButton.removeAttribute("aria-describedby");
  hideWhileSharing.checked = preferences.hideWhileSharing && (!desktop.macApp || accessibility === "granted");
  setDisabled(hideWhileSharing, !loaded || (desktop.macApp &&
    (accessibility === undefined || unsupported || accessibilityOpening)));
  setText(accessibilityStatus, translate(accessibility ? accessibilityLabels[accessibility] : "accessibilityChecking"));
  setDisabled(settingsAccessibility, accessibility === undefined || unsupported);
  setAttribute(settingsAccessibility, "aria-disabled", String(accessibilityOpening || settingsAccessibility.disabled));
  setAttribute(noticeAccessibility, "aria-disabled", String(accessibilityOpening));

  const view = activeView();
  renderPermissionNotice(view);
  const target = view.dataset.moduleView as FeedbackTarget;
  const explicit = feedback[target];
  const applying = workerAction?.running && workerAction.target === target;
  const message = applying ? undefined
    : explicit ?? (target !== "tools" && target !== "settings" && target !== "devices" ? toolError(target) : undefined);
  const toolActionFailure = !applying && target === "tools" && !explicit && toolFailures.length > 0;
  const lines: FeedbackLine[] = toolActionFailure ? [...toolFailures] : message ? [{ message }] : [];
  if (accessibilityRequestIssue === target) lines.push({ message: "accessibilityRequestFailed" });

  const title = toolActionFailure ? toolFailureTitle : lines.length && !message ? "accessibility" : target;
  const key = JSON.stringify([title, lines]);
  const open = lines.length > 0 && dismissedFeedback[target] !== key;
  if (dismissedFeedback[target] !== key) delete dismissedFeedback[target];
  if (open) {
    const description = lines.map(({ message, tool }) =>
      tool && tool !== title ? `${translate(tool)} — ${translate(message)}` : translate(message)).join("\n");
    const titleText = translate(title);
    const canOpenAccessibility = !unsupported && lines.some(({ message }) =>
      message === "permissionRequired" || message === "accessibilityRequestFailed");
    const informational = lines.every(({ message }) => calmMessages.has(message));
    if (!canOpenAccessibility && document.activeElement === noticeAccessibility) view.querySelector("h1")?.focus();
    noticeAccessibility.toggleAttribute("hidden", !canOpenAccessibility);
    setAttribute(noticeRegion, "data-tone", informational ? "info" : "error");
    setText(noticeTitle, titleText);
    setText(noticeDescription, description);
    setText(noticeAnnouncement, `${titleText}. ${description}`);
    visibleNotice = { target, key };
    if (bell && !informational && recordedFeedback[target] !== key) {
      recordedFeedback[target] = key;
      void desktop.recordNotification("error", title, { detail: lines[0].message }, target).catch(() => {});
    }
  } else {
    if (lines.length === 0 && recordedFeedback[target]) {
      // The problem is gone, so the bell no longer lists it.
      delete recordedFeedback[target];
      void desktop.dismissNotificationRoute(target, "error").catch(() => {});
    }
    if (noticeRegion.contains(document.activeElement)) view.querySelector("h1")?.focus();
    setText(noticeAnnouncement, "");
    visibleNotice = undefined;
  }
  setNoticeOpen(noticeRegion, open);
  renderBlocks();
}

/** Turns off a control with the reason as its tip. The reason shows on hover and focus, so the control keeps its place. */
function setBlock(control: HTMLElement, reason: Reason | undefined): void {
  if (reason) {
    const tip = translate(reason);
    control.classList.add("reason");
    setAttribute(control, "data-blocked", "");
    setAttribute(control, "data-tip", tip);
    setAttribute(control, "aria-description", tip);
    setAttribute(control, "aria-disabled", "true");
  } else if (control.hasAttribute("data-blocked")) {
    control.classList.remove("reason");
    for (const name of ["data-blocked", "data-tip", "aria-description"]) control.removeAttribute(name);
  }
}

/** The controls that the controlled device will refuse, so the window says so before the click. */
function renderBlocks(): void {
  if (!remote) return;
  for (const power of powerButtons) {
    const tool = power.dataset.toolPower as ToolId;
    setBlock(power, refusal("setToolActive", { tool, active: statuses[tool].mode === "off" }, session?.permissions));
  }
  for (const button of [settingsAccessibility, noticeAccessibility, permissionAccessibility]) {
    setBlock(button, refusal("requestActivityAccessibility", undefined, session?.permissions));
  }
}

function renderPermissionNotice(view = activeView()): void {
  const missingPermission = needsAccessibility();
  if (!missingPermission || permissionNotice?.trigger.closest("[data-module-view]") !== view) {
    permissionNotice = undefined;
  }
  const trigger = permissionNotice?.trigger;
  if (trigger) {
    setText(permissionTitle, translate(trigger.dataset.permissionWarning as Message));
  } else if (permissionRegion.contains(document.activeElement)) {
    view.querySelector("h1")?.focus({ preventScroll: true });
  }
  setNoticeOpen(permissionRegion, Boolean(trigger));
  setAttribute(permissionAccessibility, "aria-disabled", String(accessibilityOpening));
  for (const button of permissionWarnings) {
    button.hidden = !missingPermission;
    setAttribute(button, "aria-expanded", String(trigger === button));
  }
  positionPermissionNotice();
}

function positionPermissionNotice(): void {
  const anchor = permissionNotice?.trigger;
  if (!anchor) return;
  const bounds = anchor.getBoundingClientRect();
  const viewport = document.documentElement;
  const gutter = 8;
  const gap = 6;
  const topEdge = Math.max(0, main.getBoundingClientRect().top);
  if (bounds.bottom <= topEdge || bounds.top >= viewport.clientHeight) {
    dismissPermissionNotice();
    return;
  }

  const below = viewport.clientHeight - bounds.bottom - gap - gutter;
  const above = bounds.top - gap - topEdge - gutter;
  const height = permissionRegion.scrollHeight + permissionRegion.offsetHeight - permissionRegion.clientHeight;
  const placeBelow = height <= below || below >= above;
  const available = Math.max(0, placeBelow ? below : above);
  const top = placeBelow ? bounds.bottom + gap : bounds.top - gap - Math.min(height, available);
  const left = Math.max(gutter, Math.min(bounds.left, viewport.clientWidth - permissionRegion.offsetWidth - gutter));
  permissionRegion.style.setProperty("--anchor-top", `${top}px`);
  permissionRegion.style.setProperty("--anchor-left", `${left}px`);
  permissionRegion.style.setProperty("--anchor-height", `${available}px`);
}

function showPermissionNotice(trigger: HTMLButtonElement, pinned = false): void {
  if (!needsAccessibility()) return;
  window.clearTimeout(permissionNoticeDismissal);
  if (permissionNotice?.trigger === trigger && !pinned) {
    positionPermissionNotice();
    return;
  }
  permissionNotice = { trigger, pinned };
  renderPermissionNotice();
  if (pinned && permissionNotice) permissionAccessibility.focus({ preventScroll: true });
}

function schedulePermissionNoticeDismissal(): void {
  window.clearTimeout(permissionNoticeDismissal);
  permissionNoticeDismissal = window.setTimeout(() => {
    const notice = permissionNotice;
    if (!notice || notice.pinned ||
        notice.trigger.matches(":hover, :focus") || permissionRegion.matches(":hover, :focus-within")) return;
    dismissPermissionNotice();
  }, 300);
}

function dismissPermissionNotice(restoreFocus = false): void {
  const trigger = permissionNotice?.trigger;
  if (!trigger) return;
  if (permissionRegion.contains(document.activeElement)) {
    if (restoreFocus) trigger.focus({ preventScroll: true });
    else (document.activeElement as HTMLElement).blur();
  }
  window.clearTimeout(permissionNoticeDismissal);
  permissionNotice = undefined;
  renderPermissionNotice();
}

function dismissNotice(): void {
  if (!visibleNotice) return;
  const { target, key } = visibleNotice;
  dismissedFeedback[target] = key;
  renderFeedback();
}

function renderStatus(): void {
  for (const button of toolNavButtons) {
    const running = ready && statuses[button.dataset.view as ToolId].mode !== "off";
    button.toggleAttribute("data-running", running);
    if (running) setAttribute(button, "aria-describedby", "tool-running-description");
    else button.removeAttribute("aria-describedby");
  }
  for (const label of statusLabels) {
    const { mode } = statuses[label.dataset.toolStatus as ToolId];
    updateToolStatus(label, mode, translate(mode));
  }
  for (const timer of sessionTimers) {
    const tool = timer.dataset.toolTimer as ToolId;
    const status = statuses[tool];
    const remaining = tool === "activity" ? status.secondsLeft : null;
    const seconds = remaining ?? status.elapsedSeconds;
    if (seconds !== null) {
      const display = formatSession(seconds);
      const label = translate(remaining === null ? "elapsedTime" : "remainingTime");
      setText(timer, display);
      setAttribute(timer, "aria-label", label + " " + display);
      setAttribute(timer, "title", label);
    }
    timer.toggleAttribute("data-visible", seconds !== null);
    setAttribute(timer, "aria-hidden", String(seconds === null));
  }
  renderControls();
  renderFeedback();
}

function canChangeTool(id: ToolId, active: boolean): boolean {
  const current = statuses[id];
  return current.supported && (current.mode === "off") === active;
}

function selectActivityMode(mode: ActivityMode): void {
  activityMode = mode;
  activityModes.dataset.selection = mode;
  for (const option of modeChoices) option.checked = option.value === mode;
  for (const [panel, active] of [[activityDuration, mode === "duration"], [scheduleForm, mode === "schedule"]] as const) {
    panel.toggleAttribute("data-active", active);
    panel.toggleAttribute("inert", !active);
    setAttribute(panel, "aria-hidden", String(!active));
  }
  if (mode === "schedule") {
    setAttribute(activityPower, "aria-describedby", "schedule-description");
  } else {
    activityPower.removeAttribute("aria-describedby");
  }
}

function renderActivityControls(unavailable: boolean, pending: string): void {
  const disabled = unavailable || !statuses.activity.supported;
  for (const option of modeChoices) setDisabled(option, disabled);
  for (const button of durationButtons) setDisabled(button, disabled || activityMode !== "duration");

  const scheduleUnavailable = disabled || activityMode !== "schedule";
  const { enabled } = preferences.schedule;
  for (const field of [start, end, scheduleSave]) setDisabled(field, scheduleUnavailable);
  if (!enabled && !scheduleUnavailable && document.activeElement === scheduleDisable) scheduleSave.focus();
  setDisabled(scheduleDisable, scheduleUnavailable || !enabled);
  setAttribute(scheduleDisable, "aria-hidden", String(!enabled));
  setAttribute(scheduleSave, "aria-disabled", pending);
  setAttribute(scheduleDisable, "aria-disabled", pending);
  setText(scheduleSaveLabel, translate(enabled ? "scheduleApply" : "scheduleEnable"));
  scheduleState.toggleAttribute("data-enabled", enabled);
  const state = !enabled ? "scheduleOff" : statuses.activity.mode === "off" ? "scheduleWaiting" : "scheduleOn";
  setText(scheduleState, translate(state));
}

function renderControls(): void {
  const unavailable = !loaded || !ready;
  const pending = String(workerAction !== undefined);
  for (const power of powerButtons) {
    const current = statuses[power.dataset.toolPower as ToolId];
    setDisabled(power, unavailable || !current.supported);
    setAttribute(power, "aria-disabled", pending);
    setAttribute(power, "aria-checked", String(current.mode !== "off"));
  }
  for (const toggle of includeSwitches) {
    const tool = toggle.dataset.toolInclude as ToolId;
    setDisabled(toggle, unavailable || !statuses[tool].supported);
    if (toggle.checked !== preferences.quickStart[tool]) toggle.checked = preferences.quickStart[tool];
  }
  for (const hint of quickStartHints) setAttribute(hint, "data-quick-start-hint", quickStartHint());
  proxy.render(preferences.proxy, preferences.quickStart.proxy, statuses.proxy.mode !== "off", loaded);
  devices.render();
  updateToolOrder(translate, preferences.pinned);
  if (!workerAction?.keepQuickStartLayout) {
    const canStartSelected = !unavailable &&
      (tools.some(({ id }) => preferences.quickStart[id] && canChangeTool(id, true)) || proxy.hasQuickStart());
    const canStopSelected = !unavailable && tools.some(({ id }) => preferences.quickStart[id] && canChangeTool(id, false));
    if (canStartSelected) setDisabled(startSelected, false);
    if (canStopSelected) setDisabled(stopSelected, false);
    updateQuickStartMode(canStopSelected ? (canStartSelected ? "mixed" : "stop") : "start");
    setDisabled(startSelected, !canStartSelected);
    setDisabled(stopSelected, !canStopSelected);
  }
  setAttribute(startSelected, "aria-disabled", pending);
  setAttribute(stopSelected, "aria-disabled", pending);

  renderActivityControls(unavailable, pending);
  startToolsOnLaunch.checked = preferences.startToolsOnLaunch;
  setDisabled(startToolsOnLaunch, !loaded);
  for (const button of [...languageButtons, ...themeButtons, ...notificationModeButtons]) setDisabled(button, !loaded);
  // With the sound off, its choices keep their place and turn off.
  const soundOff = !loaded || !preferences.notifications.sound;
  for (const button of soundNameButtons) setDisabled(button, soundOff);
  setDisabled(notificationSoundPlay, soundOff);
  setDisabled(notificationSound, !loaded);
  caffeinate.setDisabled(unavailable || !statuses.caffeinate.supported);
  renderBlocks();
}

function renderTheme(animate: boolean): void {
  const theme = preferences.theme === "system"
    ? (systemAppearance.matches ? "dark" : "light")
    : preferences.theme;
  // The boot script reads this before the first paint of the next launch.
  if (loaded) {
    try {
      localStorage.setItem("lazy-worker-theme", preferences.theme);
    } catch {
      // Without storage, the next launch starts with the system theme.
    }
  }
  if (document.documentElement.dataset.theme === theme) return;
  if (animate) document.documentElement.setAttribute("data-theme-transition", "");
  document.documentElement.dataset.theme = theme;
}

function renderPreferences(previous?: Preferences): void {
  if (previous?.theme !== preferences.theme) {
    desktop.setTheme(preferences.theme);
    for (const button of themeButtons) {
      setAttribute(button, "aria-pressed", String(button.dataset.themeChoice === preferences.theme));
    }
  }
  renderTheme(previous !== undefined);
  if (previous?.duration !== preferences.duration) {
    for (const button of durationButtons) {
      setAttribute(button, "aria-pressed", String(button.dataset.duration === preferences.duration));
    }
  }
  if (JSON.stringify(previous?.notifications) !== JSON.stringify(preferences.notifications)) {
    const { mode, sound, soundName } = preferences.notifications;
    for (const button of notificationModeButtons) {
      setAttribute(button, "aria-pressed", String(button.dataset.notificationMode === mode));
    }
    for (const button of soundNameButtons) {
      setAttribute(button, "aria-pressed", String(button.dataset.soundName === soundName));
    }
    notificationSound.checked = sound;
    setCollapseOpen(notificationSoundSection, mode !== "off");
    renderNotificationPermission();
  }
  if (previous?.language !== preferences.language) {
    for (const button of languageButtons) {
      setAttribute(button, "aria-pressed", String(button.dataset.language === preferences.language));
    }
  }
  const language = preferredLanguage();
  if (renderedLanguage !== language) {
    renderedLanguage = language;
    document.documentElement.lang = language;
    translated.forEach(node => setText(node, translate(node.dataset.text as Message)));
    accessibleLabels.forEach(node => setAttribute(node, "aria-label", translate(node.dataset.label as Message)));
    translatedTips.forEach(node => {
      const tip = translate(node.dataset.tipText as Message);
      setAttribute(node, "data-tip", tip);
      // An info icon has no text of its own. A tile keeps its name and gets the tip as its description.
      if (node.classList.contains("option-info")) setAttribute(node, "aria-label", tip);
      else node.querySelector("input")?.setAttribute("aria-description", tip);
    });
    setAttribute(element("tool-navigation"), "aria-label", translate("tools"));
    activityPower.removeAttribute("aria-labelledby");
    setAttribute(activityPower, "aria-label", translate("activitySession"));
    progress.renderLabel();
    updates.render();
    if (remote) banner.render();
    onboarding.render();
    changelog.render();
    bell?.render();
    renderNotificationPermission();
    renderLid();
    renderStatus();
  } else {
    renderControls();
    renderFeedback();
  }
}

function renderPendingPreferences(): void {
  const previous = preferences;
  preferences = pendingPreferences.reduce((current, change) => change.apply(current), savedPreferences);
  renderPreferences(previous);
}

function setStatuses(current: Statuses, activityAttempted = false): void {
  if (activityAttempted) ++activityResultRevision;
  if (activityAttempted || current.activity.error !== "permissionRequired") {
    activityPermissionErrorRetired = false;
  }
  statuses = current;
}

function refreshAccessibility(): Promise<void> {
  if (accessibilityRefresh) return accessibilityRefresh;
  const revision = activityResultRevision;
  accessibilityRefresh = desktop.getActivityAccessibility()
    .catch(() => "unavailable" as const)
    .then(current => {
      accessibility = current;
      if (current === "granted" && revision === activityResultRevision) {
        activityPermissionErrorRetired = statuses.activity.error === "permissionRequired";
        if (feedback.activity === "permissionRequired") delete feedback.activity;
        toolFailures = toolFailures.filter(({ tool, message }) =>
          tool !== "activity" || message !== "permissionRequired");
        accessibilityRequestIssue = undefined;
      }
      renderFeedback();
    })
    .finally(() => { accessibilityRefresh = undefined; });
  return accessibilityRefresh;
}

const permissionChecks = [
  {
    refresh: refreshAccessibility,
    pending: () => accessibility === "required" || (desktop.macApp && activeView().dataset.moduleView === "settings") ||
      (statuses.activity.error === "permissionRequired" && !activityPermissionErrorRetired),
  },
];

function refreshPermissions(pendingOnly = false): void {
  for (const permission of permissionChecks) {
    if (!pendingOnly || permission.pending()) void permission.refresh();
  }
}

async function openAccessibility(target: FeedbackTarget): Promise<void> {
  // Only the person at the device can answer its system prompt.
  if (remote || accessibilityOpening || accessibility === "unsupported") return;
  accessibilityOpening = true;
  renderFeedback();
  try {
    await desktop.requestActivityAccessibility();
    accessibilityRequestIssue = undefined;
    refreshPermissions();
  } catch {
    accessibilityRequestIssue = target;
    delete dismissedFeedback[target];
  } finally {
    accessibilityOpening = false;
    renderFeedback();
  }
}

/** True while a status request waits. The next tick skips, so a slow link never piles requests up. */
let statusPending = false;

async function refreshStatus(): Promise<void> {
  if (workerAction?.running || caffeinateSaving || statusPending) return;
  const revision = ++statusRevision;
  statusPending = true;
  try {
    const current = await desktop.getStatuses();
    if (revision !== statusRevision) return;
    setStatuses(current);
    if (!ready && (feedback.tools === "workerUnavailable" || feedback.tools === "notPermitted")) delete feedback.tools;
    ready = true;
  } catch (error) {
    if (revision !== statusRevision) return;
    ready = false;
    // A lost link shows in the banner. A device that refuses shows a calm note.
    if (!remote) feedback.tools = "workerUnavailable";
    else if (isLinkError(error)) delete feedback.tools;
    else feedback.tools = errorMessage(error, "workerUnavailable");
  } finally {
    statusPending = false;
  }
  renderStatus();
}

function clearFeedback(target: FeedbackTarget): void {
  delete feedback[target];
  if (target === "tools") toolFailures = [];
  if (target === accessibilityRequestIssue) accessibilityRequestIssue = undefined;
}

async function updateLoginLaunch(enabled?: boolean): Promise<void> {
  if (!desktop.macApp || loginLaunchPending) return;
  loginLaunchPending = true;
  const previous = enabled === undefined ? loginLaunch.checked : !enabled;
  const wasDisabled = loginLaunch.disabled;
  loginLaunch.disabled = true;
  try {
    if (enabled === undefined) loginLaunch.checked = await desktop.getLoginLaunch();
    else await desktop.setLoginLaunch(enabled);
    loginLaunch.disabled = false;
  } catch (error) {
    loginLaunch.checked = previous;
    loginLaunch.disabled = wasDisabled;
    feedback.settings = errorMessage(error, enabled === undefined ? "loginLaunchCheckFailed" : "loginLaunchSaveFailed");
    delete dismissedFeedback.settings;
    renderFeedback();
  } finally {
    loginLaunchPending = false;
  }
}

type SaveStep = { changed: boolean; send: () => Promise<void>; done: (saved: Preferences) => Preferences };

/** Sends each changed setting as its own request, in a fixed order, and keeps what was saved if a later one fails. */
async function save(next: Preferences): Promise<void> {
  const before = savedPreferences;
  const differs = (pick: (preferences: Preferences) => unknown) =>
    JSON.stringify(pick(next)) !== JSON.stringify(pick(before));
  const choice = <Key extends PreferenceChoice | "schedule" | "pinned">(
    key: Key, send: (value: Preferences[Key]) => Promise<void>,
  ): SaveStep => ({
    changed: differs(preferences => preferences[key]),
    send: () => send(next[key]),
    done: saved => ({ ...saved, [key]: next[key] }),
  });
  const proxyPart = <Key extends keyof ProxySettings>(
    key: Key, send: (value: ProxySettings[Key]) => Promise<void>,
  ): SaveStep => ({
    changed: differs(preferences => preferences.proxy[key]),
    send: () => send(next.proxy[key]),
    done: saved => ({ ...saved, proxy: { ...saved.proxy, [key]: next.proxy[key] } }),
  });
  const steps: SaveStep[] = [
    choice("language", desktop.saveLanguage),
    choice("theme", desktop.saveTheme),
    choice("duration", desktop.saveDuration),
    choice("schedule", desktop.saveSchedule),
    ...tools.map(({ id }): SaveStep => ({
      changed: next.quickStart[id] !== before.quickStart[id],
      send: () => desktop.saveQuickStart(id, next.quickStart[id]),
      done: saved => ({ ...saved, quickStart: { ...saved.quickStart, [id]: next.quickStart[id] } }),
    })),
    choice("startToolsOnLaunch", desktop.saveStartToolsOnLaunch),
    choice("hideWhileSharing", desktop.saveHideWhileSharing),
    choice("pinned", desktop.savePinnedTools),
    choice("seenVersion", desktop.saveSeenVersion),
    choice("notifications", desktop.saveNotifications),
    proxyPart("role", desktop.saveProxyRole),
    proxyPart("port", desktop.saveProxyPort),
    proxyPart("server", desktop.saveProxyServer),
    proxyPart("allowed", desktop.saveProxyAllowed),
    proxyPart("apps", desktop.saveProxyApps),
  ];
  for (const { changed, send, done } of steps) {
    if (!changed) continue;
    await send();
    savedPreferences = done(savedPreferences);
  }
}

function updatePreference(target: FeedbackTarget, change: PreferenceChange): void {
  clearFeedback(target);
  pendingPreferences.push(change);
  renderPendingPreferences();
  queue = queue.then(async () => {
    try {
      await save(change.apply(savedPreferences));
    } catch (error) {
      if (!pendingPreferences.some(pending => pending !== change && pending.key === change.key)) {
        feedback[target] = errorMessage(error, "saveFailed");
        delete dismissedFeedback[target];
      }
    } finally {
      pendingPreferences.shift();
      renderPendingPreferences();
    }
  });
}

function updateChoice<Key extends PreferenceChoice>(
  target: FeedbackTarget, key: Key, value: Preferences[Key],
): void {
  if (preferences[key] !== value) {
    updatePreference(target, { key, apply: current => ({ ...current, [key]: value }) });
  }
}

function updateWorker(
  target: FeedbackTarget,
  action: (requested: Preferences) => Promise<void>,
  { fallback = "saveFailed", keepQuickStartLayout = false }: {
    fallback?: Message;
    keepQuickStartLayout?: boolean;
  } = {},
): void {
  if (!loaded || !ready || workerAction) return;
  const requested = preferences;
  const operation = { target, running: false, keepQuickStartLayout };
  workerAction = operation;
  clearFeedback(target);
  renderControls();
  renderFeedback();
  queue = queue.then(async () => {
    operation.running = true;
    ++statusRevision;
    try {
      await action(requested);
    } catch (error) {
      feedback[target] = errorMessage(error, fallback);
    } finally {
      workerAction = undefined;
      delete dismissedFeedback[target];
      renderPendingPreferences();
      renderStatus();
    }
  });
}

function saveSchedule(enabled: boolean): void {
  if (workerAction) return;
  const next = enabled
    ? { enabled, start: start.value, end: end.value }
    : { ...preferences.schedule, enabled };
  if (enabled && !scheduleForm.reportValidity()) return;
  if (enabled && next.start === next.end) {
    feedback.activity = "invalidSchedule";
    delete dismissedFeedback.activity;
    end.focus();
    renderFeedback();
    return;
  }
  updateWorker("activity", async () => {
    await progress.run([{
      label: () => translate("applyingChanges"),
      run: async () => {
        const previous = savedPreferences.schedule;
        const rescheduled = enabled && (!previous.enabled || previous.start !== next.start || previous.end !== next.end);
        await save({ ...savedPreferences, schedule: next });
        setStatuses(await desktop.getStatuses(), rescheduled);
      },
    }]);
    if (enabled && !toolError("activity")) feedback.activity = "saved";
  });
}

function cancelCaffeinateAutosave(): void {
  ++caffeinateEditRevision;
  window.clearTimeout(caffeinateSaveDelay);
  caffeinateSaveDelay = undefined;
}

async function applyCaffeinateSettings(settings: CaffeinateSettings): Promise<void> {
  if (JSON.stringify(settings) === JSON.stringify(savedPreferences.caffeinate) &&
    statuses.caffeinate.error !== "caffeinateReconfigureFailed") return;
  caffeinateSaving = true;
  ++statusRevision;
  try {
    setStatuses(await desktop.saveCaffeinateSettings(settings));
    savedPreferences = { ...savedPreferences, caffeinate: settings };
    toolFailures = toolFailures.filter(({ tool }) => tool !== "caffeinate");
  } finally {
    caffeinateSaving = false;
  }
}

function queueCaffeinateAutosave(): void {
  if (!ready || !loaded) return;
  cancelCaffeinateAutosave();
  caffeinateSaveDelay = window.setTimeout(flushCaffeinateAutosave, 700);
}

function flushCaffeinateAutosave(): void {
  if (caffeinateSaveDelay === undefined) return;
  window.clearTimeout(caffeinateSaveDelay);
  caffeinateSaveDelay = undefined;
  const edit = caffeinateEditRevision;
  queue = queue.then(async () => {
    if (edit !== caffeinateEditRevision) return;
    clearFeedback("caffeinate");
    delete dismissedFeedback.caffeinate;
    try {
      const settings = caffeinate.settingsForSave(savedPreferences.caffeinate);
      // A saved setting is silent. The notice shows only while a running session takes the change.
      if (statuses.caffeinate.mode === "off") await applyCaffeinateSettings(settings);
      else {
        await progress.run([{
          label: () => translate("applyingChanges"),
          run: () => applyCaffeinateSettings(settings),
        }]);
      }
    } catch (error) {
      const incomplete = error === "caffeinateInvalidPid" || error === "caffeinateInvalidDuration";
      if (edit === caffeinateEditRevision && !incomplete) {
        feedback.caffeinate = errorMessage(error, "saveFailed");
      }
    } finally {
      renderPendingPreferences();
      renderStatus();
    }
  });
}

function toolActionLabel(id: ToolId, active: boolean): string {
  return translate(active ? "toolStarting" : "toolStopping").replace("{tool}", translate(id));
}

async function setToolActive(id: ToolId, active: boolean): Promise<void> {
  if (id === "caffeinate" && active) {
    cancelCaffeinateAutosave();
    await applyCaffeinateSettings(caffeinate.settingsForSave(savedPreferences.caffeinate));
  }
  setStatuses(await desktop.setToolActive(id, active), id === "activity" && active);
  if (id === "activity" && active) refreshPermissions();
}

async function runToolChanges(ids: readonly ToolId[], active: boolean, failureTitle: Message): Promise<void> {
  toolFailureTitle = failureTitle;
  for (const id of ids) clearFeedback(id);
  await progress.run(ids.map(id => ({
    label: () => toolActionLabel(id, active),
    run: async () => {
      let message: Message | undefined;
      if (!canChangeTool(id, active)) return;
      try {
        await setToolActive(id, active);
        message = toolError(id);
      } catch (error) {
        message = errorMessage(error, active ? startFailures[id] : "workerUnavailable");
      }
      delete dismissedFeedback[id];
      if (message) toolFailures.push({ tool: id, message });
      renderStatus();
    },
  })));
}

let pointerPress: { element: HTMLButtonElement; id: number } | undefined;

function clearPointerPress(): void {
  pointerPress?.element.removeAttribute("data-press-held");
  pointerPress = undefined;
}

for (const type of ["pointerup", "pointercancel"] as const) {
  window.addEventListener(type, event => {
    if (pointerPress?.id === event.pointerId) clearPointerPress();
  }, true);
}
window.addEventListener("blur", clearPointerPress);

for (const button of [startSelected, stopSelected, ...powerButtons]) {
  let releaseFeedback: number | undefined;
  const allowed = () => !button.disabled && button.getAttribute("aria-disabled") !== "true";
  const activationKey = (event: KeyboardEvent) => event.key === " " || event.key === "Enter";
  button.addEventListener("pointerdown", event => {
    if (!allowed() || !event.isPrimary || event.button !== 0) return;
    clearPointerPress();
    pointerPress = { element: button, id: event.pointerId };
    button.setAttribute("data-press-held", "");
  });
  button.addEventListener("keydown", event => {
    if (!activationKey(event)) return;
    if (event.repeat) event.preventDefault();
    if (allowed()) button.setAttribute("data-press-held", "");
  });
  button.addEventListener("keyup", event => {
    if (activationKey(event)) button.removeAttribute("data-press-held");
  });
  button.addEventListener("blur", () => button.removeAttribute("data-press-held"));
  button.addEventListener("click", () => {
    if (!allowed()) return;
    window.clearTimeout(releaseFeedback);
    button.setAttribute("data-press-feedback", "");
    releaseFeedback = window.setTimeout(() => button.removeAttribute("data-press-feedback"), 250);
  });
}

powerButtons.forEach(button => button.addEventListener("click", () => {
  if (button.hasAttribute("data-blocked")) return;
  const id = button.dataset.toolPower as ToolId;
  const active = statuses[id].mode === "off";
  const target = button.closest('[data-module-view="tools"]') ? "tools" : id;
  if (target === "activity" && active && needsAccessibility()) {
    showPermissionNotice(activityWarning, true);
    return;
  }
  updateWorker(target, async requested => {
    if (id === "activity" && active && requested.duration !== savedPreferences.duration) throw "saveFailed";
    if (target === "tools") {
      await runToolChanges([id], active, id);
      return;
    }
    await progress.run([{
      label: () => id === "caffeinate" && active ? translate("caffeinateStarting") : toolActionLabel(id, active),
      run: async () => {
        await setToolActive(id, active);
        toolFailures = toolFailures.filter(({ tool }) => tool !== id);
        renderStatus();
      },
    }]);
  }, { fallback: active ? startFailures[id] : "workerUnavailable" });
}));
includeSwitches.forEach(toggle => toggle.addEventListener("change", () => {
  const id = toggle.dataset.toolInclude as ToolId;
  const checked = toggle.checked;
  const target = toggle.closest('[data-module-view="tools"]') ? "tools" : id;
  updatePreference(target, {
    key: id,
    apply: current => ({ ...current, quickStart: { ...current.quickStart, [id]: checked } }),
  });
}));
function changeSelected(active: boolean): void {
  updateWorker("tools", async requested => {
    if (tools.some(({ id }) => requested.quickStart[id] !== savedPreferences.quickStart[id])) throw "saveFailed";
    setStatuses(await desktop.getStatuses());
    const ids = tools.filter(({ id }) => requested.quickStart[id] && canChangeTool(id, active)).map(({ id }) => id);
    if (active && ids.includes("activity") && requested.duration !== savedPreferences.duration) throw "saveFailed";
    await runToolChanges(ids, active, active ? "selectedStartFailed" : "selectedStopFailed");
    if (active) await proxy.launchQuickStart();
  }, { fallback: "workerUnavailable", keepQuickStartLayout: true });
}

startSelected.addEventListener("click", () => changeSelected(true));
stopSelected.addEventListener("click", () => changeSelected(false));
durationButtons.forEach(button => button.addEventListener("click", () => {
  updateChoice("activity", "duration", button.dataset.duration as Duration);
}));
languageButtons.forEach(button => button.addEventListener("click", () => {
  updateChoice("settings", "language", button.dataset.language as Preferences["language"]);
}));
themeButtons.forEach(button => button.addEventListener("click", () => {
  updateChoice("settings", "theme", button.dataset.themeChoice as Theme);
}));
function renderNotificationPermission(): void {
  const state = notificationPermissionState;
  const shown = !remote && preferences.notifications.mode === "system" && (state === "notDetermined" || state === "denied");
  // The note keeps its last text while it closes.
  if (shown) {
    notificationPermissionBox.dataset.state = state;
    setText(notificationPermissionText, translate(state === "denied" ? "notificationsBlocked" : "notificationsAsk"));
    setText(notificationPermissionAction, translate(state === "denied" ? "notificationsOpenSettings" : "notificationsAllow"));
  }
  notificationPermissionAction.toggleAttribute("data-attention", shown && state === "denied");
  setCollapseOpen(notificationPermission, shown);
}
async function refreshNotificationPermission(): Promise<void> {
  if (remote) return;
  notificationPermissionState = await desktop.getNotificationPermission().catch(() => "unknown" as const);
  renderNotificationPermission();
}
notificationPermissionAction.addEventListener("click", () => {
  const blocked = notificationPermissionState === "denied";
  const action = blocked ? desktop.openNotificationSettings() : desktop.requestNotificationPermission();
  void action.then(() => refreshNotificationPermission()).catch(error => {
    feedback.settings = errorMessage(error, "notificationSettingsFailed");
    delete dismissedFeedback.settings;
    renderFeedback();
  });
});
if (!remote) {
  void desktop.onNotificationPermission(() => void refreshNotificationPermission());
  void refreshNotificationPermission();
}
function updateNotifications(change: Partial<Preferences["notifications"]>): void {
  updatePreference("settings", {
    key: "notifications",
    apply: current => ({ ...current, notifications: { ...current.notifications, ...change } }),
  });
}
notificationModeButtons.forEach(button => button.addEventListener("click", () => {
  const mode = button.dataset.notificationMode as NotificationMode;
  if (preferences.notifications.mode !== mode) updateNotifications({ mode });
}));
soundNameButtons.forEach(button => button.addEventListener("click", () => {
  const soundName = button.dataset.soundName as SoundName;
  void desktop.playNotificationSound(soundName).catch(() => {});
  if (preferences.notifications.soundName !== soundName) updateNotifications({ soundName });
}));
notificationSound.addEventListener("change", () => updateNotifications({ sound: notificationSound.checked }));
notificationSoundPlay.addEventListener("click", () => {
  void desktop.playNotificationSound(preferences.notifications.soundName).catch(() => {});
});
loginLaunch.addEventListener("change", () => {
  clearFeedback("settings");
  renderFeedback();
  void updateLoginLaunch(loginLaunch.checked);
});
startToolsOnLaunch.addEventListener("change", () => {
  updateChoice("settings", "startToolsOnLaunch", startToolsOnLaunch.checked);
});
hideWhileSharing.addEventListener("change", () => {
  if (hideWhileSharing.checked && desktop.macApp && accessibility !== "granted") {
    hideWhileSharing.checked = false;
    showPermissionNotice(sharingWarning, true);
    return;
  }
  updateChoice("settings", "hideWhileSharing", hideWhileSharing.checked);
});
modeChoices.forEach(option => option.addEventListener("change", () => {
  if (!option.checked) return;
  selectActivityMode(option.value as ActivityMode);
  renderControls();
}));
scheduleDisable.addEventListener("click", () => saveSchedule(false));
scheduleForm.addEventListener("submit", event => {
  event.preventDefault();
  saveSchedule(true);
});
caffeinate.onChange(queueCaffeinateAutosave);
for (const button of permissionWarnings) {
  button.addEventListener("pointerenter", event => {
    if (event.pointerType !== "touch") showPermissionNotice(button);
  });
  button.addEventListener("focus", () => showPermissionNotice(button));
  button.addEventListener("click", () => showPermissionNotice(button, true));
  button.addEventListener("pointerleave", schedulePermissionNoticeDismissal);
  button.addEventListener("blur", schedulePermissionNoticeDismissal);
  button.addEventListener("keydown", event => {
    if (event.key !== "Tab" || event.shiftKey || button.getAttribute("aria-expanded") !== "true") return;
    event.preventDefault();
    permissionAccessibility.focus({ preventScroll: true });
  });
}
permissionRegion.addEventListener("pointerenter", () => window.clearTimeout(permissionNoticeDismissal));
permissionRegion.addEventListener("pointerleave", schedulePermissionNoticeDismissal);
permissionRegion.addEventListener("focusout", schedulePermissionNoticeDismissal);
window.addEventListener("resize", positionPermissionNotice);
window.addEventListener("scroll", positionPermissionNotice, { capture: true, passive: true });
function dismissPermissionNoticeOutside(event: Event): void {
  const trigger = permissionNotice?.trigger;
  const target = event.target;
  if (trigger && target instanceof Node && !trigger.contains(target) && !permissionRegion.contains(target)) {
    dismissPermissionNotice();
  }
}
document.addEventListener("click", dismissPermissionNoticeOutside, true);
document.addEventListener("focusin", dismissPermissionNoticeOutside);
document.addEventListener("keydown", event => {
  if (event.key === "Escape") dismissPermissionNotice(true);
});
permissionDismiss.addEventListener("click", () => dismissPermissionNotice(true));
permissionAccessibility.addEventListener("click", () => {
  dismissPermissionNotice(true);
  void openAccessibility(activeView().dataset.moduleView as FeedbackTarget);
});
noticeDismiss.addEventListener("click", dismissNotice);
noticeAccessibility.addEventListener("click", () => {
  if (visibleNotice) void openAccessibility(visibleNotice.target);
});
settingsAccessibility.addEventListener("click", () => { void openAccessibility("settings"); });
for (const button of document.querySelectorAll<HTMLButtonElement>("[data-view]")) {
  button.addEventListener("click", () => {
    renderFeedback();
    // Opening a place answers what the bell said about it.
    if (!remote && button.dataset.view) void desktop.dismissNotificationRoute(button.dataset.view, "error").catch(() => {});
    if (!remote && button.dataset.view === "settings") void desktop.dismissNotificationRoute("settings", "update").catch(() => {});
    if (button.dataset.view === "settings") {
      refreshPermissions();
      void updateLoginLaunch();
      void refreshNotificationPermission();
    }
    if (button.dataset.view === "devices") void devices.refresh();
    else devices.reset();
  });
}
async function refreshSystemLanguage(): Promise<void> {
  const language = await desktop.getSystemLanguage();
  if (language === systemLanguage) return;
  systemLanguage = language;
  renderPreferences(preferences);
}

systemAppearance.addEventListener("change", () => renderTheme(true));
// The panel is an app, not a page: a right click offers nothing, except the editing menu of a text
// field and Copy for text the person selected.
document.addEventListener("contextmenu", event => {
  const target = event.target instanceof Element ? event.target : null;
  const editable = target?.closest("input, textarea, [contenteditable=\"true\"]");
  const selected = (window.getSelection()?.toString() ?? "") !== "";
  if (!editable && !selected) event.preventDefault();
});
window.addEventListener("languagechange", () => { void refreshSystemLanguage(); });
desktop.onFocus(() => {
  proxy.refresh();
  if (!remote && activeView().dataset.moduleView === "devices") void devices.refresh();
  refreshPermissions();
  void refreshLid();
  if (activeView().dataset.moduleView === "settings") {
    void updateLoginLaunch();
    void refreshNotificationPermission();
  }
  void refreshSystemLanguage();
  if (remote) void refreshViewer();
  else void updates.refresh();
});

/** What the device keeps and what the viewer sees. The language and theme are the viewer's, whatever the device has saved. */
async function readRemote(): Promise<void> {
  const [next, device, viewer] = await Promise.all([
    desktop.getSession(),
    desktop.getPreferences().catch((error: unknown) => {
      if (isLinkError(error)) throw error;
      // A device that does not share its settings still lets this window show its tools.
      return defaults;
    }),
    desktop.getViewer(),
  ]);
  session = next;
  remoteName = next.device.name;
  banner.setSession(next);
  savedPreferences = { ...device, language: viewer.language, theme: viewer.theme };
  setText(element("app-version"), next.version);
  element("settings-version").hidden = false;
  if (document.title !== `${remoteName} — Lazy Worker`) document.title = `${remoteName} — Lazy Worker`;
}

/** Reads the session and the settings again after the link came back. A change in progress keeps the settings it has. */
async function reloadRemote(): Promise<void> {
  try {
    const keep = pendingPreferences.length > 0 || caffeinateSaveDelay !== undefined;
    const before = savedPreferences;
    await readRemote();
    if (keep) savedPreferences = before;
    preferences = pendingPreferences.reduce((current, change) => change.apply(current), savedPreferences);
    renderPreferences(before);
  } catch {
    // The link failed again. The banner says so, and the next return tries again.
  }
}

/** Follows the language and theme of the person here, which can change in the panel while this window is open. */
async function refreshViewer(): Promise<void> {
  const viewer = await desktop.getViewer().catch(() => undefined);
  if (!viewer || (viewer.language === savedPreferences.language && viewer.theme === savedPreferences.theme)) return;
  savedPreferences = { ...savedPreferences, ...viewer };
  renderPendingPreferences();
}

/** Opens the window of a device: the first read waits for the link, which can take a few seconds or fail. */
async function loadRemote(): Promise<void> {
  remoteName = await desktop.remoteName();
  banner.render();
  // The window shows at once, with its banner, so a device that does not answer is seen and not a blank panel.
  delete element("onboarding").closest<HTMLElement>(".workspace")!.dataset.booting;
  systemLanguage = await desktop.getSystemLanguage();
  for (;;) {
    try {
      await readRemote();
      preferences = savedPreferences;
      loaded = true;
      return;
    } catch {
      // The banner shows the lost link. The window keeps trying until the device answers.
      await new Promise(resolve => window.setTimeout(resolve, 5000));
    }
  }
}

async function initialize(): Promise<void> {
  renderTheme(false);
  renderStatus();
  if (!remote) {
    void desktop.getVersion().then(version => {
      setText(element("app-version"), version);
      element("settings-version").hidden = false;
    }).catch(() => {});
  }
  try {
    if (remote) {
      void desktop.getVersion().then(version => banner.setLocalVersion(version)).catch(() => {});
      await loadRemote();
    } else {
      [savedPreferences, systemLanguage] = await Promise.all([
        desktop.getPreferences(),
        desktop.getSystemLanguage(),
      ]);
      preferences = savedPreferences;
      loaded = true;
    }
    selectActivityMode(preferences.schedule.enabled ? "schedule" : "duration");
    start.value = preferences.schedule.start;
    end.value = preferences.schedule.end;
    caffeinate.set(preferences.caffeinate);
    desktop.startPreviewTools(preferences);
  } catch {
    feedback.tools = "saveFailed";
  }
  renderPreferences();
  void refreshLid();
  // The setup guide, What’s new, and the update notice belong to this app, so a window of a device shows none of them.
  if (!remote) {
    void devices.refresh();
    runningVersion = await desktop.getVersion().catch(() => undefined);
    if (await desktop.getFirstRun().catch(() => false)) onboarding.open();
    else if (loaded && runningVersion && preferences.seenVersion !== runningVersion) {
      const news = releasesSince(preferences.seenVersion, runningVersion);
      if (news.length) changelog.open(news);
      else markVersionSeen();
    }
  }
  // The panel stays hidden until here, so a new installation never shows the tools before the setup guide.
  delete element("onboarding").closest<HTMLElement>(".workspace")!.dataset.booting;
  if (!remote) void updates.start();
  await refreshStatus();
  refreshPermissions();
  window.setInterval(() => { void refreshStatus(); }, 1000);
  window.setInterval(() => refreshPermissions(true), 1800);
}

void initialize();
