import "./devices.css";
import {
  desktop, type ActionSchedule, type Controller, type ControllerPermission, type DeviceKind, type DevicesOverview,
  type DiscoveredDevice, type KeysWarning, type ProxyNetwork, type RemoteApp, type RemoteDevice, type RemoteLogEntry, type RemoteTool,
  type SavedAction, type ScreenSharing, type ShellState, type Statuses, type MailboxParty, type MailboxSummary,
} from "./desktop";
import { mountActionWindows, type Target } from "./device-actions";
import { mountFilesExplorer } from "./device-explorer";
import { mailboxBlocked } from "./mailbox";
import { mountFilesGrantWindow } from "./device-files";
import { mountShellWindow } from "./device-shell";
import { formatBytes, type FilesGrant, type FileShare } from "./files";
import guideImage13 from "./assets/guide/remote-login-13.png";
import { messages, type Language, type Message } from "./messages";
import { tools } from "./modules";
import { h, icon, type Child } from "./proxy";
import { enterNode, fadeText, only, setHidden } from "./motion";
import { mountTabs } from "./tabs";
import { isLinkError, isRefusal, refusal, type Session } from "./remote";
import { age, formatClock, formatCountdown, formatMoment, formatSession, type AgeUnit } from "./time";

// Every name and text below comes from data that other devices will send, so it is only ever set as text.
const openKey = "lazy-worker-devices-open";
/** How often the app checks whether each paired device answers, while the view shows, in milliseconds. */
const reachPoll = 5000;
/** How often the log is read while its tab shows, in milliseconds. */
const logPoll = 3000;
/** The spinner of the Refresh button shows at least this long, so the check can be seen. */
const refreshShown = 500;
/** How long a change of tab takes: the panels crossfade while the height moves. */
/** How many applications a device can have pinned, and so how many rows its section shows. */
const pinLimit = 3;
/** The applications that the tab of a device lists before the rest go behind the count: the pinned ones, then the latest used. */
const appRows = 5;
/** How many applications each device remembers a time of use for. */
const recentLimit = 24;
/** How often the applications of an open device are read, in milliseconds. */
const appPoll = 5000;
/** How long an application gets to show as open after the device accepted the request. */
const openSettle = 1000;
const closeDelay = 300;
/** How often the tools of an open device are read, in milliseconds. */
const toolPoll = 2000;
/** How often the session of an open device is read again, in milliseconds. It tells what the device allows and whether its shell is on. */
const sessionPoll = 5000;
/** How long the confirmation of a copied command shows, in milliseconds. */
const copiedFor = 1400;
/** How often the actions of an open device are read, in milliseconds. */
const actionPoll = 5000;
const settleDelay = 260;
const ageMessages: Record<Exclude<AgeUnit, "seconds">, Message> = {
  minutes: "minutesAgo", hours: "hoursAgo", days: "daysAgo",
};
/** The least width of a name in a controller row, in pixels. Keep it equal to the `min-width` of `.controller-row .device-row-text`. */
const MIN_NAME_WIDTH = 120;
/** `hint` says in one sentence what the permission allows. It shows in the pairing window and as the tip of the chip. */
const permissions: { id: ControllerPermission; icon: string; label: Message; hint?: Message }[] = [
  { id: "control", icon: "layout-grid", label: "permControl", hint: "permControlHint" },
  { id: "applications", icon: "app-window", label: "permApplications", hint: "permApplicationsHint" },
  { id: "actions", icon: "play", label: "permActions", hint: "permActionsHint" },
  { id: "mailbox", icon: "mail", label: "permMailbox", hint: "permMailboxHint" },
  { id: "files", icon: "folder", label: "permFiles", hint: "permFilesHint" },
  { id: "shell", icon: "terminal", label: "permShell" },
  { id: "screen", icon: "monitor", label: "permScreen", hint: "permScreenHint" },
];
/** The outcomes that the log shows in words. Any other one shows as it is. */
const logOutcomes: Record<string, Message> = {
  ok: "remoteOutcomeOk", notPermitted: "remoteOutcomeNotPermitted", localOnly: "remoteOutcomeLocalOnly",
  notPaired: "remoteOutcomeNotPaired", badRequest: "remoteOutcomeBadRequest", failed: "remoteOutcomeFailed",
  stopped: "remoteOutcomeStopped", timedOut: "remoteOutcomeTimedOut", skipped: "remoteOutcomeSkipped",
};
/** The codes that a failed pairing can end with, in plain words. Any other code shows the generic text. */
const pairErrors: Record<string, Message> = {
  wrongCode: "pairWrongCode", pairingCodeInvalid: "pairCodeShort", pairingClosed: "pairClosedThere", pairingBusy: "pairBusy",
  pairingHostMismatch: "pairHostMismatch", pairingFailed: "pairFailed", pairingSaveFailed: "pairSaveFailed", pairingSelf: "pairSelf",
  sshUnreachable: "pairUnreachable", sshDenied: "pairDenied", sshTimeout: "pairTimeout", pairingTimeout: "pairTimeout",
  sshHostKeyChanged: "pairHostKeyChanged", sshHostKeyUnknown: "pairHostKeyUnknown", destinationInvalid: "addressInvalid",
};
/** After these the window does not let the user press Pair again until the user goes back or changes a field. */
const stopErrors = new Set(["pairingHostMismatch", "sshHostKeyChanged"]);
const kindIcons: Record<DeviceKind, string> = { laptop: "laptop", desktop: "monitor" };
const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");

type Host = {
  translate(key: Message): string;
  language(): Language;
  refreshIcons(): void;
  /** Shows the notice of a failed change. `error` is the code that the app sent, when it sent one. */
  fail(error: unknown): void;
  /** Shows a message that is not a failure. */
  notify(message: Message): void;
  /** Opens the mailbox with a device. `online` is nothing for a device that controls this one. */
  openMailbox(party: MailboxParty, name: string, online: boolean | null, trigger: HTMLElement): void;
};
type Row<T> = { root: HTMLElement; update(item: T): void };
type AppItem = RemoteApp & { id: string };
type AppsState = {
  /** What the device itself said about the Applications permission, once it has answered. */
  permitted?: boolean;
  apps?: RemoteApp[];
  /** The code that ended the last read. */
  error?: string;
  /** The applications that were asked to open and have not settled yet. */
  opening: Set<string>;
};
type ActionsState = {
  /** What the device itself said about the Actions permission, once it has answered. */
  permitted?: boolean;
  actions?: SavedAction[];
  /** The code that ended the last read. */
  error?: string;
};
type WarningItem = Omit<KeysWarning, "id"> & { id: string };
/** What the last read of a device found. The session is read less often than the tools, because it rarely changes. */
type LiveDevice = {
  tools?: RemoteTool[]; error?: string; permissions?: Session["permissions"]; shell?: ShellState;
  /** What the device shares with this one, and the code that ended the last read of it. */
  files?: FileShare; filesError?: string;
  /** What the last read of its Screen Sharing found, and the code that ended it when it failed. */
  screen?: ScreenSharing; screenError?: string;
};
type Item = { id: string; self: boolean; name: string; kind: DeviceKind; data: DevicesOverview["thisDevice"] | RemoteDevice };

/** Replaces `{name}` markers. A function does the replacing, so a `$` in a name stays as it is. */
function fill(text: string, values: Record<string, string | number>): string {
  return text.replace(/\{(\w+)\}/g, (marker, key: string) => key in values ? String(values[key]) : marker);
}

function setText(node: HTMLElement, text: string): void {
  if (node.textContent !== text) node.textContent = text;
}

function setAttribute(node: HTMLElement, name: string, value: string): void {
  if (node.getAttribute(name) !== value) node.setAttribute(name, value);
}

type Stored = { open: Record<string, boolean>; tabs: Record<string, string>; recent: Record<string, Record<string, number>> };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * The open device and the last tab of each device are a per-viewer convenience: the view works without storage.
 * Older data was one flat map of rows and sections to booleans. Its section keys have no use now, and are dropped.
 */
function readStored(): Stored {
  try {
    const stored: unknown = JSON.parse(localStorage.getItem(openKey) ?? "{}");
    if (!isRecord(stored)) return { open: {}, tabs: {}, recent: {} };
    const nested = isRecord(stored.open) || isRecord(stored.tabs);
    const open = nested ? stored.open : stored;
    const tabs = nested ? stored.tabs : {};
    return {
      open: Object.fromEntries(Object.entries(isRecord(open) ? open : {})
        .filter(([key, value]) => typeof value === "boolean" && !key.includes("/"))) as Record<string, boolean>,
      tabs: Object.fromEntries(Object.entries(isRecord(tabs) ? tabs : {})
        .filter(([, value]) => typeof value === "string")) as Record<string, string>,
      recent: Object.fromEntries(Object.entries(isRecord(stored.recent) ? stored.recent : {}).flatMap(([device, apps]) => isRecord(apps)
        ? [[device, Object.fromEntries(Object.entries(apps).filter(([, time]) => typeof time === "number"))]] : [])) as Record<string, Record<string, number>>,
    };
  } catch {
    return { open: {}, tabs: {}, recent: {} };
  }
}

/** At most one device is open. The last one that was opened stays open, and `this` counts as the oldest. */
function oneOpen(stored: Record<string, boolean>): Record<string, boolean> {
  const keys = Object.keys(stored);
  if (!keys.includes("this")) keys.unshift("this");
  const next = { ...stored };
  let open: string | undefined;
  for (const key of keys) {
    if (!(stored[key] ?? key === "this")) continue;
    if (open !== undefined) next[open] = false;
    open = key;
  }
  return next;
}

export function mountDevices(host: Host) {
  const t = host.translate;
  const deviceList = document.getElementById("device-list")!;
  // The heading of the view has a backing only while the list scrolls under it.
  const heading = document.querySelector<HTMLElement>(".devices-header")!;
  const scroller = document.querySelector("main")!;
  scroller.addEventListener("scroll", () => heading.toggleAttribute("data-stuck", scroller.scrollTop > 12), { passive: true });
  const addButton = document.getElementById("add-device") as HTMLButtonElement;
  const addTip = addButton.parentElement!;

  const appsWindow = document.getElementById("device-apps-window")!;
  const appsList = document.getElementById("device-apps-list")!;
  const appsCount = document.getElementById("device-apps-count")!;
  const appsClose = document.getElementById("device-apps-close") as HTMLButtonElement;
  const appsSearch = document.getElementById("device-apps-search") as HTMLInputElement;

  const guideWindow = document.getElementById("remote-guide-window")!;
  const guideTitle = document.getElementById("remote-guide-title")!;
  const guideBody = document.getElementById("remote-guide-body")!;
  const guideClose = document.getElementById("remote-guide-close") as HTMLButtonElement;
  const guideOpen = document.getElementById("remote-guide-open") as HTMLButtonElement;

  let overview: DevicesOverview | undefined;
  /** What the mailbox knows of each conversation: new items, and whether a device allows it. */
  let mailboxes: MailboxSummary[] = [];
  const mailboxOf = (party: MailboxParty) => mailboxes.find(entry =>
    "device" in party ? "device" in entry.party && entry.party.device === party.device : "peer" in entry.party && entry.party.peer === party.peer);
  let enabling = false;
  /** What this device found about its own Screen Sharing. Nothing until the first answer. */
  let localScreen: ScreenSharing | undefined;
  let enablingScreen = false;
  /** Which service the guide window explains. */
  let guideKind: "login" | "screen" = "login";
  let guideTrigger: HTMLElement | undefined;
  let requested = 0;
  /** What waits for a second click: a device id, or `cut` for all access. Only one thing waits at a time. */
  let confirming: string | undefined;
  /** Set after Cut all access, until a device is paired again. Remote Login may still be on. */
  let afterCut = false;
  let logEntries: RemoteLogEntry[] = [];
  /** Changes when the entries or the language change, so a list that shows an older version draws again. */
  let logVersion = 0;
  let logTicker: number | undefined;
  const storedState = readStored();
  let opened = oneOpen(storedState.open);
  /** The last tab of each device, by key. */
  let tabChoices = storedState.tabs;
  /** When an application was last opened from here, by device and bundle identifier. */
  let recentApps = storedState.recent as Record<string, Record<string, number>>;
  /** How each device row opens and closes, by key. */
  const disclosures = new Map<string, (open: boolean, animate: boolean) => void>();
  let built = false;
  let sequence = 0;
  let shownLanguage: Language | undefined;
  /** What the last read of each device's tools found. `error` is the code that ended the last read, if one did. */
  const live = new Map<string, LiveDevice>();
  const polling = new Set<string>();
  let toolTicker: number | undefined;
  const readingSessions = new Set<string>();
  const readingScreens = new Set<string>();
  const readingFiles = new Set<string>();
  let sessionTicker: number | undefined;
  const appsStates = new Map<string, AppsState>();
  const readingApps = new Set<string>();
  let appTicker: number | undefined;
  let appsDevice: string | undefined;
  let appsTrigger: HTMLElement | undefined;
  let appsSignature = "";
  let appsRows = new Map<string, WindowRow>();
  const appsNote = h("li", { class: "setting-note", hidden: true });
  /** The actions of this device, and of each device that this one controls once its section is open. */
  let localActions: SavedAction[] = [];
  let localActionsError: string | undefined;
  const actionStates = new Map<string, ActionsState>();
  const readingActions = new Set<string>();
  let actionTicker: number | undefined;
  /** The devices whose reachability is being checked now, so a slow check is never started twice. */
  const reaching = new Set<string>();
  /** The devices whose Refresh button was pressed and has not finished. */
  const refreshing = new Set<string>();
  let reachTicker: number | undefined;

  function isOpen(key: string, fallback = false): boolean {
    return opened[key] ?? fallback;
  }

  function persist(): void {
    try {
      localStorage.setItem(openKey, JSON.stringify({ open: opened, tabs: tabChoices, recent: recentApps }));
    } catch {
      // Without storage the rows still open and close. They only start closed next time.
    }
  }

  function remember(key: string, open: boolean): void {
    // The key moves to the end, so the order of the keys is the order in which they were last changed.
    const { [key]: _previous, ...rest } = opened;
    opened = { ...rest, [key]: open };
    persist();
  }

  /** Opens or closes one device. Opening it closes the other open one, and both move over a short time. */
  function setOpen(key: string, open: boolean, animate = true): void {
    remember(key, open);
    if (open) {
      for (const other of new Set([...Object.keys(opened), ...disclosures.keys(), "this"])) {
        if (other === key || !isOpen(other, other === "this")) continue;
        remember(other, false);
        disclosures.get(other)?.(false, animate);
      }
    }
    disclosures.get(key)?.(open, animate);
  }

  /** The tab that shows in a device now: the last one chosen, or the first when that one is not a tab of the device. */
  function tabOf(key: string, ids: readonly string[]): string {
    const chosen = tabChoices[key];
    return chosen !== undefined && ids.includes(chosen) ? chosen : ids[0];
  }

  /** True when the device is open and shows this tab. A device that has no stored choice shows its first tab. */
  function showsTab(key: string, id: string, first: string): boolean {
    return isOpen(key, key === "this") && (tabChoices[key] ?? first) === id;
  }

  function seconds(): number {
    return Date.now() / 1000;
  }

  function elapsed(since: number): string {
    const { value, unit } = age(seconds() - since);
    return unit === "seconds" ? t("justNow") : fill(t(ageMessages[unit]), { n: value });
  }

  function setTip(root: HTMLElement, text: string): void {
    setAttribute(root, "data-tip", text);
  }

  /** A name on one line. It ends in an ellipsis when it does not fit, and a tip then shows the whole name. */
  function oneLine(name: HTMLElement): HTMLElement {
    const root = h("span", { class: "tip name-tip" }, name);
    root.addEventListener("pointerenter", () => {
      if (name.scrollWidth > name.clientWidth) root.setAttribute("data-tip", name.textContent ?? "");
      else root.removeAttribute("data-tip");
    });
    return root;
  }

  /** Texts of the windows. They are set again when the language changes. */
  const labelled: (() => void)[] = [];
  function bind<T extends HTMLElement>(node: T, key: Message, attribute?: string): T {
    const apply = (): void => {
      if (attribute) setAttribute(node, attribute, t(key));
      else setText(node, t(key));
    };
    apply();
    labelled.push(apply);
    return node;
  }

  function bindTip(control: { root: HTMLElement; button: HTMLButtonElement }, key: Message): void {
    labelled.push(() => {
      setTip(control.root, t(key));
      setAttribute(control.button, "aria-label", t(key));
    });
  }

  const windows = mountActionWindows({
    translate: key => host.translate(key),
    language: () => host.language(),
    refreshIcons: () => host.refreshIcons(),
    fail: error => host.fail(error),
    changed: () => void refreshActions(),
  });

  const shells = mountShellWindow({
    translate: key => host.translate(key),
    language: () => host.language(),
    refreshIcons: () => host.refreshIcons(),
    fail: error => host.fail(error),
    changed: () => void refresh(),
  });

  const fileGrants = mountFilesGrantWindow({
    translate: key => host.translate(key),
    language: () => host.language(),
    refreshIcons: () => host.refreshIcons(),
    fail: error => host.fail(error),
    changed: () => void refresh(),
    home: () => overview?.thisDevice.home ?? null,
  });

  const explorer = mountFilesExplorer({
    translate: key => host.translate(key),
    language: () => host.language(),
    refreshIcons: () => host.refreshIcons(),
    fail: error => host.fail(error),
  });

  /** An icon-only button with its name as a tip. The wrapper carries the tip, so a disabled button can still give a reason. */
  function iconButton(label: string, name: string, extra = "") {
    const button = h("button", { type: "button", class: `icon-button ${extra}`.trim(), "aria-label": label }, icon(name));
    const root = h("span", { class: "tip", "data-tip": label }, button);
    return {
      root,
      button,
      /** Turns the button off with the reason as its tip, or back on with its name. */
      disable(reason: string | undefined, name = label): void {
        const off = reason !== undefined;
        if (button.disabled !== off) button.disabled = off;
        setTip(root, reason ?? name);
      },
    };
  }

  /** The mailbox button of a row, with the count of what is new. */
  function mailboxButton() {
    const base = iconButton(t("mailbox"), "mail", "outlined mailbox-button");
    const count = h("span", { class: "mailbox-count", "aria-hidden": "true", hidden: true });
    base.button.append(count);
    base.button.setAttribute("aria-haspopup", "dialog");
    return {
      ...base,
      /** Shows the count of what is new. With `blocked`, the button is off and its wrapper's tip says why. */
      set(unread: number, blocked?: string): void {
        setHidden(count, unread === 0);
        const text = unread > 99 ? "99+" : String(unread);
        if (count.textContent !== text) count.textContent = text;
        const label = unread ? fill(t("mailboxUnread"), { n: unread }) : t("mailbox");
        base.disable(blocked, label);
        setAttribute(base.button, "aria-label", blocked ? `${label}. ${blocked}` : label);
      },
    };
  }

  function playButton(label: string, tip: string) {
    const button = h("button", { type: "button", class: "round-button", "aria-label": label }, icon("play"));
    return { root: h("span", { class: "tip", "data-tip": tip }, button), button };
  }

  /** A closed note that opens over a short time. The last text stays while it closes. */
  function note(text: string, warning = false) {
    const paragraph = h("p", { class: "setting-note" }, text);
    if (warning) paragraph.setAttribute("data-warning", "");
    const root = h("div", { class: "collapse", inert: true }, h("div", {}, paragraph));
    return {
      root,
      set(value: string): void {
        fadeText(paragraph, value);
      },
      show(open: boolean): void {
        root.toggleAttribute("data-open", open);
        root.toggleAttribute("inert", !open);
      },
    };
  }

  /** The row that opens and closes `body`. A button over the whole row keeps the row's own buttons clickable. */
  function disclosure(key: string, head: HTMLElement, label: HTMLElement, body: HTMLElement, fallback = false): HTMLButtonElement {
    const id = `device-panel-${++sequence}`;
    label.id = `${id}-label`;
    body.id = id;
    const toggle = h("button", {
      type: "button", class: "device-toggle", "aria-labelledby": label.id, "aria-controls": id,
    });
    // A tall body clips its content while it opens, so it does not cover the rows under it. Then tips can show past its edge.
    body.setAttribute("data-settle", "");
    let settling: number | undefined;
    const apply = (open: boolean, animate: boolean): void => {
      window.clearTimeout(settling);
      head.toggleAttribute("data-open", open);
      body.toggleAttribute("data-open", open);
      body.toggleAttribute("inert", !open);
      toggle.setAttribute("aria-expanded", String(open));
      if (open && animate && !reducedMotion.matches) {
        body.removeAttribute("data-settled");
        settling = window.setTimeout(() => body.setAttribute("data-settled", ""), settleDelay);
      } else {
        body.toggleAttribute("data-settled", open);
      }
    };
    disclosures.set(key, apply);
    apply(isOpen(key, fallback), false);
    toggle.addEventListener("click", () => {
      setOpen(key, !body.hasAttribute("data-open"));
      // An opened device reads itself now. A closed one stops the reading.
      watchTools(true);
      watchApps(true);
      watchActions(true);
      watchSessions(true);
      watchLog(true);
      const id = key.startsWith("r:") ? key.slice(2) : undefined;
      if (id !== undefined && body.hasAttribute("data-open") && !reaching.has(id)) void checkReach(id);
    });
    head.prepend(toggle);
    return toggle;
  }

  /**
   * The body of a tab: a scroll area that fills the panel under its header line. A message takes the area's place and is
   * centered in it, and the content under it cannot be reached while it shows.
   */
  function tabBody(...content: (Node | false)[]) {
    const scroll = h("div", { class: "tab-scroll" }, ...content);
    const message = h("p", { class: "tab-empty", "aria-hidden": "true" });
    const root = h("div", { class: "tab-body" }, scroll, message);
    return {
      root,
      scroll,
      /** Shows `text` in place of the content. An empty text brings the content back. */
      say(text: string, warning = false): void {
        const showing = text !== "";
        if (showing) fadeText(message, text);
        message.toggleAttribute("data-warning", warning);
        root.toggleAttribute("data-empty", showing);
        scroll.inert = showing;
        setAttribute(message, "aria-hidden", String(!showing));
        message.setAttribute("role", showing ? "status" : "presentation");
      },
    };
  }

  /** The header line of a tab and the area under it. */
  function tabColumn(body: ReturnType<typeof tabBody>, head?: HTMLElement): HTMLElement {
    return h("div", { class: "tab-column" }, ...(head ? [head] : []), body.root);
  }

  /** Fields that hold typed input. They are emptied when the user leaves the view. */
  const searches = new Set<() => boolean>();

  /** A search field for the header line of a tab. `onChange` runs as the user types and when the field is cleared. */
  function searchField(label: Message, onChange: () => void) {
    const input = h("input", { class: "field", type: "text", autocomplete: "off", spellcheck: "false" });
    const clear = iconButton(t("clearSearch"), "x");
    clear.root.hidden = true;
    const root = h("div", { class: "tab-search" }, icon("search"), input, clear.root);
    const change = (): void => {
      clear.root.hidden = input.value === "";
      onChange();
    };
    input.addEventListener("input", change);
    input.addEventListener("keydown", event => {
      if (event.key === "Escape" && input.value !== "") {
        event.stopPropagation();
        input.value = "";
        change();
      }
    });
    clear.button.addEventListener("click", () => {
      input.value = "";
      change();
      input.focus();
    });
    bind(input, label, "placeholder");
    bind(input, label, "aria-label");
    labelled.push(() => {
      setTip(clear.root, t("clearSearch"));
      setAttribute(clear.button, "aria-label", t("clearSearch"));
    });
    searches.add(() => {
      if (input.value === "") return false;
      input.value = "";
      clear.root.hidden = true;
      return true;
    });
    return { root, query: (): string => input.value.trim().toLocaleLowerCase() };
  }

  type TabSpec = { id: string; title: Message; panel: HTMLElement };

  /**
   * The tabs of a device: a bar that looks like the app's segmented choices, and one visible panel under it.
   * The choice is kept for the device. `mountTabs` gives the bar its behavior.
   */
  function tabset(key: string, label: string, specs: readonly TabSpec[], summaries: boolean, onSelect: (id: string) => void) {
    const base = `device-tabs-${++sequence}`;
    const bar = h("div", { class: "segments tab-segments", role: "tablist", "aria-label": label });
    bar.toggleAttribute("data-summaries", summaries);
    const container = h("div", { class: "tab-panels" });
    for (const spec of specs) {
      spec.panel.classList.add("tab-panel");
      spec.panel.setAttribute("role", "tabpanel");
      spec.panel.id = `${base}-${spec.id}`;
      spec.panel.setAttribute("aria-labelledby", `${base}-${spec.id}-tab`);
      container.append(spec.panel);
      bar.append(h("button", {
        type: "button", role: "tab", id: `${base}-${spec.id}-tab`, "aria-controls": spec.panel.id, "data-tab": spec.id,
        "aria-selected": "false", tabindex: "-1",
      }, h("span", { class: "tab-title" }, t(spec.title)), h("span", { class: "tab-summary" })));
    }
    bar.append(h("span", { class: "segment-indicator", "aria-hidden": "true" }));
    const tabs = mountTabs(bar, specs.map(spec => spec.panel), {
      initial: tabOf(key, specs.map(spec => spec.id)),
      onSelect: id => {
        tabChoices = { ...tabChoices, [key]: id };
        persist();
        onSelect(id);
      },
    });
    return {
      root: h("div", { class: "device-tabs", "data-device": key === "this" ? "this" : "other" }, bar, container),
      selected: tabs.selected,
      summary: tabs.summary,
    };
  }

  /**
   * Keeps one element for each row of a list and updates it in place. A row is rebuilt when its signature
   * changes, which is its name, its icon, or the language. A new row opens and a removed row closes over a short time.
   */
  function list<T extends { id: string }>(
    parent: HTMLElement, signature: (item: T) => string, build: (item: T) => Row<T>,
  ) {
    const entries = new Map<string, { signature: string; shell: HTMLElement; row: Row<T> }>();
    let first = true;
    return {
      sync(items: readonly T[]): void {
        const animate = !first && !reducedMotion.matches && !parent.closest("[hidden]");
        first = false;
        const seen = new Set<string>();
        let previous: HTMLElement | undefined;
        for (const item of items) {
          seen.add(item.id);
          const current = signature(item);
          let entry = entries.get(item.id);
          let created = false;
          if (entry && entry.signature !== current) {
            entry.shell.remove();
            entries.delete(item.id);
            entry = undefined;
          }
          if (!entry) {
            const row = build(item);
            const shell = h("div", { class: "collapse" }, h("div", {}, row.root));
            entry = { signature: current, shell, row };
            entries.set(item.id, entry);
            created = true;
            built = true;
          }
          entry.row.update(item);
          const { shell } = entry;
          const expected = previous ? previous.nextElementSibling : parent.firstElementChild;
          if (shell !== expected) parent.insertBefore(shell, expected);
          if (created) {
            // A new row starts closed and opens on the next frame, so the rows under it move instead of jumping.
            if (animate) {
              void shell.offsetHeight;
            }
            shell.setAttribute("data-open", "");
          }
          previous = shell;
        }
        for (const [id, { shell }] of entries) {
          if (seen.has(id)) continue;
          entries.delete(id);
          shell.removeAttribute("data-open");
          shell.setAttribute("inert", "");
          if (animate) window.setTimeout(() => shell.remove(), closeDelay);
          else shell.remove();
        }
      },
    };
  }

  /** The actions whose name has `query` in it. The query is already lowercase. */
  function filterActions(actions: readonly SavedAction[], query: string): SavedAction[] {
    return query === "" ? [...actions] : actions.filter(action =>
      action.name.toLocaleLowerCase().includes(query) || (action.description ?? "").toLocaleLowerCase().includes(query));
  }

  /** `2 actions · next 02:00`, `1 action`, or `No actions`. */
  function actionCount(list: readonly SavedAction[]): string {
    const base = list.length === 0 ? t("actionCountNone")
      : list.length === 1 ? t("actionCountOne") : fill(t("actionCountMany"), { count: list.length });
    const next = list.flatMap(action => action.nextRun === null ? [] : [action.nextRun]);
    return next.length ? `${base} · ${fill(t("actionsNextSummary"), { time: formatClock(Math.min(...next), host.language()) })}` : base;
  }

  function weekday(day: number): string {
    const index = Number.isFinite(day) ? ((Math.trunc(day) % 7) + 7) % 7 : 0;
    // 7 January 2024 is a Sunday.
    return new Date(2024, 0, 7 + index).toLocaleDateString(host.language(), { weekday: "long" });
  }

  function scheduleText(schedule: ActionSchedule | null): string {
    if (schedule === null) return t("actionManual");
    if (schedule.kind === "daily") return fill(t("actionDaily"), { time: schedule.time });
    if (schedule.kind === "weekly") return fill(t("actionWeekly"), { day: weekday(schedule.day), time: schedule.time });
    return fill(t("actionOnce"), { time: formatClock(schedule.at, host.language()) });
  }

  /** The schedule, who set it when this is the device itself, and how the last run went. */
  function actionText(action: SavedAction, own: boolean): string {
    const parts = [scheduleText(action.schedule)];
    if (own && action.schedule && action.scheduledBy) {
      parts.push(action.scheduledBy === "local" ? t("actionSetHere")
        : action.scheduledByName ? fill(t("actionSetBy"), { name: action.scheduledByName }) : t("actionSetByOther"));
    }
    const last = action.lastRun;
    if (last) {
      const key: Message = last.ok ? "actionLastOk" : last.timedOut ? "actionLastTimedOut" : "actionLastFailed";
      parts.push(fill(t(key), { age: elapsed(last.end) }));
    }
    return parts.join(" · ");
  }

  function kindIcon(kind: DeviceKind): HTMLElement {
    const node = icon(kindIcons[kind] ?? "monitor");
    node.classList.add("device-kind");
    return node;
  }

  function chevron(): HTMLElement {
    const node = icon("chevron-right");
    node.classList.add("device-chevron");
    return node;
  }

  /** Runs an action and opens the window of its result. A device that refuses, or an action that is busy, says so as a notice. */
  async function startRun(target: Target, action: SavedAction, trigger: HTMLElement): Promise<void> {
    try {
      const { runId } = await desktop.runAction(target.id, action.id);
      windows.openRun(target, action, runId, trigger);
    } catch (error) {
      if (error === "notPermitted" && target.id !== null) actionsOf(target.id).permitted = false;
      else host.fail(error);
    }
    void refreshActions();
  }

  async function deleteAction(action: SavedAction): Promise<void> {
    confirming = undefined;
    try {
      await desktop.deleteAction(action.id);
    } catch (error) {
      host.fail(error);
    }
    await readLocalActions();
  }

  /** The row of an action. Its own device also edits and deletes it. Another device only runs and schedules it. */
  function actionRow(device: { id: string; name: string } | undefined): (action: SavedAction) => Row<SavedAction> {
    const target = (): Target => device ?? { id: null, name: "" };
    return action => {
      let latest = action;
      const name = h("strong", {}, action.name);
      const description = h("span", { class: "setting-note action-description" });
      const summary = h("span", { class: "setting-note" });
      const edit = !device && iconButton(fill(t("editAction"), { action: action.name }), "pencil", "outlined");
      const schedule = iconButton(fill(t("scheduleAction"), { action: action.name }), "calendar-clock", "outlined");
      schedule.button.setAttribute("aria-haspopup", "dialog");
      const remove = !device && iconButton(fill(t("deleteAction"), { action: action.name }), "trash-2");
      const run = appOpenButton();
      const confirm = remove && h("div", { class: "device-confirm", hidden: true });
      const buttons = remove && confirmButtons(t("deleteActionAccept"), () => { void deleteAction(latest); }, () => {
        confirming = undefined;
        render();
        remove.button.focus();
      });
      if (confirm && buttons) confirm.append(h("span", { class: "setting-note" }, t("deleteActionConfirm")), buttons.accept.root, buttons.cancel.root);
      const controls = h("div", { class: "device-end" },
        edit && edit.root, schedule.root, remove && remove.root, confirm, run.root);
      const root = h("div", { class: "device-row" },
        h("span", { class: "action-icon" }, icon("square-terminal")),
        h("div", { class: "device-row-text" }, oneLine(name), description, summary), controls);
      edit && edit.button.addEventListener("click", () => windows.openForm(latest, edit.button));
      schedule.button.addEventListener("click", () => windows.openSchedule(target(), latest, schedule.button));
      run.button.addEventListener("click", () => void startRun(target(), latest, run.button));
      if (remove && buttons) {
        remove.button.addEventListener("click", () => {
          confirming = `action:${latest.id}`;
          render();
          buttons.cancel.button.focus();
        });
      }
      return {
        root,
        update(current) {
          latest = current;
          const about = current.description ?? "";
          setHidden(description, about === "");
          if (about !== "") fadeText(description, about);
          fadeText(summary, actionText(current, device === undefined));
          root.toggleAttribute("data-failed", current.notifyOnFailure && current.lastRun !== null && !current.lastRun.ok);
          const label = fill(t("runAction"), { action: current.name });
          run.set(label, current.busy ? t("runRunning") : t("runNow"), current.busy);
          if (run.button.disabled !== current.busy) run.button.disabled = current.busy;
          if (confirm && edit && remove) {
            const controls = [edit.root, schedule.root, remove.root, run.root];
            if (confirming === `action:${current.id}`) only([confirm, ...controls], [confirm]);
            else only([confirm, ...controls], controls);
          }
        },
      };
    };
  }

  const actionSignature = (action: SavedAction): string => JSON.stringify([host.language(), action.name]);

  /** Saves the three permissions that can change here. The shell is turned on at the device itself. */
  async function setPermission(controller: Controller, id: ControllerPermission, on: boolean): Promise<void> {
    const next = { ...controller.permissions, [id]: on };
    try {
      await desktop.setPeerPermissions(controller.id, {
        control: next.control, applications: next.applications, actions: next.actions, screen: next.screen,
      });
    } catch (error) {
      host.fail(error);
    }
    await refresh();
  }

  /** The mailbox has its own switch. Turning it off drops what waits for that device. */
  async function setMailbox(controller: Controller, on: boolean): Promise<void> {
    try {
      await desktop.setPeerMailbox(controller.id, on);
    } catch (error) {
      host.fail(error);
    }
    await refresh();
  }

  async function removeController(controller: Controller): Promise<void> {
    confirming = undefined;
    try {
      await desktop.removePeer(controller.id);
    } catch (error) {
      host.fail(error);
    }
    await refresh();
  }

  async function forgetRemote(device: RemoteDevice): Promise<void> {
    confirming = undefined;
    try {
      const { removeOnDevice } = await desktop.forgetDevice(device.id);
      const { [device.id]: _forgotten, ...rest } = recentApps;
      recentApps = rest;
      persist();
      // This device cannot take its own key out of the other one, so the user has to do it there.
      if (removeOnDevice) host.notify("forgetRemoveThere");
    } catch (error) {
      host.fail(error);
    }
    await refresh();
  }

  function confirmButtons(yesLabel: string, yes: () => void, no: () => void) {
    const accept = iconButton(yesLabel, "check", "danger");
    const cancel = iconButton(t("cancel"), "x");
    accept.button.addEventListener("click", yes);
    cancel.button.addEventListener("click", no);
    return { accept, cancel };
  }

  /** Ends the shell of a device now. */
  async function revokeShell(controller: Controller): Promise<void> {
    confirming = undefined;
    try {
      await desktop.revokeShell(controller.id);
    } catch (error) {
      host.fail(error);
    }
    await refresh();
  }

  /** Stops sharing files with a device now. Its transfers in progress end. */
  async function revokeFiles(controller: Controller): Promise<void> {
    confirming = undefined;
    try {
      await desktop.revokeFiles(controller.id);
    } catch (error) {
      host.fail(error);
    }
    await refresh();
  }

  /** `until 17:00` for files that have an end, and `until off` for those that do not. */
  function filesChipText(files: FilesGrant): string {
    return files.until === "untilOff" ? t("permFilesUntilOff")
      : fill(t("permFilesUntil"), { time: formatClock(files.until, host.language()) });
  }

  /** `until 17:00` for a shell that has an end, and `until off` for one that does not. */
  function shellChipText(shell: NonNullable<ShellState>): string {
    return shell.until === "untilOff" ? t("permShellUntilOff")
      : fill(t("permShellUntil"), { time: formatClock(shell.until, host.language()) });
  }

  function controllerRow(controller: Controller): Row<Controller> {
    let current = controller;
    const detail = h("span", { class: "setting-note" });
    const shellConfirm = confirmButtons(t("shellTurnOff"), () => { void revokeShell(current); }, () => {
      confirming = undefined;
      render();
      shellChip?.focus();
    });
    let shellChip: HTMLButtonElement | undefined;
    const filesConfirm = confirmButtons(t("filesTurnOff"), () => { void revokeFiles(current); }, () => {
      confirming = undefined;
      render();
      filesChip?.focus();
    });
    let filesChip: HTMLButtonElement | undefined;
    const mailboxConfirm = confirmButtons(t("mailboxTurnOff"), () => {
      confirming = undefined;
      void setMailbox(current, false);
    }, () => {
      confirming = undefined;
      render();
      mailboxChip?.focus();
    });
    mailboxConfirm.cancel.root.dataset.tip = t("mailboxKeep");
    mailboxConfirm.cancel.button.setAttribute("aria-label", t("mailboxKeep"));
    let mailboxChip: HTMLButtonElement | undefined;
    const chips = permissions.map(permission => {
      const label = h("span", { class: "chip-label" }, t(permission.label));
      // The time that a shell or files end at, as a short suffix. The label and the suffix stay on one line.
      const suffix = h("span", { class: "chip-suffix" });
      const chip = h("button", {
        type: "button", class: "permission-chip", "aria-pressed": "false", "aria-label": t(permission.label),
      }, icon(permission.icon), label, suffix);
      const wrapper = h("span", { class: "tip" }, chip);
      if (permission.id === "shell") shellChip = chip;
      if (permission.id === "files") filesChip = chip;
      if (permission.id === "mailbox") mailboxChip = chip;
      chip.addEventListener("click", () => {
        const on = chip.getAttribute("aria-pressed") === "true";
        if (permission.id === "mailbox") {
          // Turning it off deletes what waits for that device, so the row asks first when something does.
          if (on && (mailboxOf({ peer: current.id })?.pending ?? 0) > 0) {
            confirming = `mailbox:${current.id}`;
            render();
            mailboxConfirm.cancel.button.focus();
          } else {
            void setMailbox(current, !on);
          }
        } else if (permission.id !== "shell" && permission.id !== "files") {
          void setPermission(current, permission.id, !on);
        } else if (on) {
          // Turning the shell or the files off asks first, in the row.
          confirming = `${permission.id}:${current.id}`;
          render();
          (permission.id === "shell" ? shellConfirm : filesConfirm).cancel.button.focus();
        } else if (permission.id === "shell") {
          shells.open({ id: current.id, name: current.name }, chip);
        } else {
          fileGrants.open({ id: current.id, name: current.name }, chip);
        }
      });
      return { permission, chip, suffix, wrapper };
    });
    // Each chip says what it allows, and with a short chip its name, in its tip. The shell has its own window, which says it.
    const chipBox = h("div", { class: "permission-chips" }, ...chips.map(({ wrapper }) => wrapper));
    const mailbox = mailboxButton();
    mailbox.button.addEventListener("click", () => host.openMailbox({ peer: current.id }, current.name, null, mailbox.button));
    const remove = iconButton(fill(t("removeController"), { device: controller.name }), "user-minus");
    const { accept, cancel } = confirmButtons(t("removeAccess"), () => { void removeController(current); }, () => {
      confirming = undefined;
      render();
      remove.button.focus();
    });
    const confirm = h("div", { class: "device-confirm", hidden: true },
      h("span", { class: "setting-note" }, t("removeConfirm")), accept.root, cancel.root);
    const offConfirm = h("div", { class: "device-confirm", hidden: true },
      h("span", { class: "setting-note" }, t("shellOffConfirm")), shellConfirm.accept.root, shellConfirm.cancel.root);
    const filesOffConfirm = h("div", { class: "device-confirm", hidden: true },
      h("span", { class: "setting-note" }, t("filesOffConfirm")), filesConfirm.accept.root, filesConfirm.cancel.root);
    const mailboxOffText = h("span", { class: "setting-note" });
    const mailboxOffConfirm = h("div", { class: "device-confirm", hidden: true },
      mailboxOffText, mailboxConfirm.accept.root, mailboxConfirm.cancel.root);
    remove.button.addEventListener("click", () => {
      confirming = current.id;
      render();
      cancel.button.focus();
    });
    const text = h("div", { class: "device-row-text" }, oneLine(h("strong", {}, controller.name)), detail);
    const root = h("div", { class: "device-row controller-row" },
      kindIcon(controller.kind),
      text,
      chipBox, mailbox.root, remove.root, confirm, offConfirm, filesOffConfirm, mailboxOffConfirm);
    /** Chips show their words while the row has room for them and the name keeps its least width. Otherwise they show their icons and times. */
    const fit = (): void => {
      if (!root.isConnected || root.clientWidth === 0) return;
      root.removeAttribute("data-compact");
      // The name column has a least width. When the chips push it down to it, they have no room for their words.
      if (text.offsetWidth <= MIN_NAME_WIDTH) root.setAttribute("data-compact", "");
    };
    new ResizeObserver(fit).observe(root);
    return {
      root,
      update(next) {
        current = next;
        fadeText(detail, next.connected ? t("connectedNow")
          : next.lastAccess === null ? t("noAccessYet") : fill(t("lastAccess"), { age: elapsed(next.lastAccess) }));
        for (const { permission, chip, suffix, wrapper } of chips) {
          const on = next.permissions[permission.id] === true;
          setAttribute(chip, "aria-pressed", String(on));
          chip.toggleAttribute("data-on", on);
          const name = t(permission.label);
          const until = permission.id === "shell" ? next.shell?.until : permission.id === "files" ? next.files?.until : undefined;
          const full = until === undefined ? name
            : permission.id === "shell" ? shellChipText(next.shell!) : filesChipText(next.files!);
          const when = until === undefined ? "" : until === "untilOff" ? "∞" : formatClock(until, host.language());
          fadeText(suffix, when);
          // A date and a time (`11/25, 02:00`) take the room of the word, which the tip and the label of the button keep.
          chip.toggleAttribute("data-long", when.length > 5);
          setAttribute(chip, "aria-label", full);
          setTip(wrapper, permission.hint ? `${full}. ${t(permission.hint)}` : full);
        }
        fit();
        // The times fade to their new text, so the width is measured once more after they did.
        window.setTimeout(fit, 400);
        const removing = confirming === next.id;
        const turningOff = confirming === `shell:${next.id}`;
        const turningFilesOff = confirming === `files:${next.id}`;
        const turningMailboxOff = confirming === `mailbox:${next.id}`;
        fadeText(mailboxOffText, fill(t("mailboxOffConfirm"), { n: mailboxOf({ peer: next.id })?.pending ?? 0 }));
        const normal = [chipBox, mailbox.root, remove.root];
        only([...normal, confirm, offConfirm, filesOffConfirm, mailboxOffConfirm],
          removing ? [confirm] : turningOff ? [offConfirm] : turningFilesOff ? [filesOffConfirm]
            : turningMailboxOff ? [mailboxOffConfirm] : normal);
        // The mailbox shows once the person here turned it on for that device. Its place stays, so the chips of every row line up.
        mailbox.root.toggleAttribute("data-absent", !next.permissions.mailbox);
        mailbox.button.inert = !next.permissions.mailbox;
        mailbox.set(mailboxOf({ peer: next.id })?.unread ?? 0);
      },
    };
  }

  /** A change to the keys file that a shell may have made. It stays until the person dismisses it. */
  function warningRow(warning: WarningItem): Row<WarningItem> {
    let current = warning;
    const dismiss = iconButton(t("keysDismiss"), "x");
    dismiss.button.addEventListener("click", () => {
      void desktop.dismissKeysWarning(Number(current.id)).catch(error => host.fail(error)).then(() => refresh());
    });
    const text = h("span", { class: "setting-note" });
    return {
      root: h("div", { class: "device-row keys-warning", role: "alert" },
        icon("triangle-alert"), h("div", { class: "device-row-text" }, text), dismiss.root),
      update(next) {
        current = next;
        setText(text, `${fill(t("keysChanged"), { name: next.name })} · ${formatMoment(next.time, host.language())}`);
      },
    };
  }

  function settingRow(name: string, label: Message, ...end: Child[]) {
    const text = h("span", { class: "setting-label" }, t(label));
    return { text, root: h("div", { class: "setting-row" }, icon(name), text, h("div", { class: "device-end" }, ...end)) };
  }

  function thisDeviceBody() {
    const accessInfo = h("span", { class: "tip option-info", tabindex: "0", role: "img" }, icon("info"));
    const accessLabel = h("span", { class: "setting-label" }, t("remoteAccess"), " ", accessInfo);
    const warnIcon = h("span", { class: "access-icon", "data-kind": "warn" }, icon("triangle-alert"));
    const okIcon = h("span", { class: "access-icon", "data-kind": "ok" }, icon("check"));
    const accessMessage = h("span", {});
    const accessNote = h("p", { class: "setting-note access-note" }, warnIcon, okIcon, accessMessage);
    // After Cut all access, Remote Login may stay on. The note says so, and the button opens Sharing settings.
    const cutNote = h("p", { class: "setting-note access-note", role: "status", hidden: true }, bind(h("span"), "cutAllRemoteLoginOn"));
    const enableLabel = h("span", {}, t("remoteAccessEnable"));
    const enable = h("button", { type: "button", class: "access-action", "data-attention": "" }, icon("power"), enableLabel);
    const sharing = iconButton(t("openSharingSettings"), "square-arrow-out-up-right", "outlined");
    const passwords = iconButton(t("allowPasswordsAgain"), "shield-off");
    const passwordButtons = confirmButtons(t("allowPasswordsAccept"), () => { void allowPasswords(); }, () => {
      confirming = undefined;
      render();
      passwords.button.focus();
    });
    const passwordConfirm = h("div", { class: "device-confirm", hidden: true },
      h("span", { class: "setting-note" }, t("allowPasswordsConfirm")), passwordButtons.accept.root, passwordButtons.cancel.root);
    const access = {
      root: h("div", { class: "setting-row device-access" },
        icon("shield-check"),
        h("div", { class: "access-text" }, accessLabel, accessNote, cutNote),
        h("div", { class: "device-end" }, enable, sharing.root, passwords.root, passwordConfirm)),
    };
    enable.addEventListener("click", () => { void enableAccess(enable); });
    sharing.button.addEventListener("click", () => { void openSharing(); });
    passwords.button.addEventListener("click", () => {
      confirming = "passwords";
      render();
      passwordButtons.cancel.button.focus();
    });
    const screenInfo = h("span", { class: "tip option-info", tabindex: "0", role: "img" }, icon("info"));
    const screenLabel = h("span", { class: "setting-label" }, t("screenSharing"), " ", screenInfo);
    const screenWarn = h("span", { class: "access-icon", "data-kind": "warn" }, icon("triangle-alert"));
    const screenOk = h("span", { class: "access-icon", "data-kind": "ok" }, icon("check"));
    const screenMessage = h("span", {});
    const screenNote = h("p", { class: "setting-note access-note" }, screenWarn, screenOk, screenMessage);
    const screenEnableLabel = h("span", {}, t("screenSharingEnable"));
    const screenEnable = h("button", { type: "button", class: "access-action", "data-attention": "" }, icon("power"), screenEnableLabel);
    const screenSettings = iconButton(t("openSharingSettings"), "square-arrow-out-up-right", "outlined");
    const screenRow = h("div", { class: "setting-row device-access" },
      icon("monitor"),
      h("div", { class: "access-text" }, screenLabel, screenNote),
      h("div", { class: "device-end" }, screenEnable, screenSettings.root));
    screenEnable.addEventListener("click", () => { void enableScreen(screenEnable); });
    screenSettings.button.addEventListener("click", () => { void openScreenSettings(); });
    const pairingLabel = h("span", { class: "setting-label" }, t("addDeviceTitle"));
    const pairingState = h("p", { class: "setting-note pairing-state" });
    const pairingButtonLabel = h("span", {}, t("pairButton"));
    const pairingButton = h("button", { type: "button", class: "access-action", "aria-haspopup": "dialog" },
      icon("lock-keyhole-open"), pairingButtonLabel);
    const pairingTip = h("span", { class: "tip" }, pairingButton);
    const pairingRow = h("div", { class: "setting-row device-access" },
      icon("lock-keyhole"),
      h("div", { class: "access-text" }, pairingLabel, pairingState),
      h("div", { class: "device-end" }, pairingTip));
    pairingButton.addEventListener("click", () => openPairWindow(pairingButton));

    const socketNote = note("");
    const controllerRows = h("div", { class: "device-rows" });
    const controllerList = list<Controller>(
      controllerRows, controller => JSON.stringify([host.language(), controller.name, controller.kind]), controllerRow);
    const warningRows = h("div", { class: "device-rows" });
    const warningList = list<WarningItem>(warningRows, warning => JSON.stringify([host.language(), warning.name]), warningRow);

    const actionsSummary = h("span", { class: "section-summary" });
    const actionSearch = searchField("searchActions", () => render());
    const addAction = iconButton(t("addAction"), "plus", "outlined");
    addAction.button.setAttribute("aria-haspopup", "dialog");
    addAction.button.addEventListener("click", () => windows.openForm(undefined, addAction.button));
    const actionRows = h("div", { class: "device-rows" });
    const actionsBody = tabBody(actionRows);
    const actionList = list<SavedAction>(actionRows, actionSignature, actionRow(undefined));

    const logCount = h("span", { class: "setting-note" });
    const logList = h("ol", { class: "proxy-log device-log-list", "data-mode": "log" });
    const logBody = tabBody(logList, bind(h("p", { class: "setting-note" }), "remoteLogNote"));
    const controllersBody = tabBody(controllerRows);
    let logDrawn = -1;

    const cutLabel = h("span", { class: "setting-label" }, t("cutAll"));
    const cutButton = h("button", { type: "button", class: "setting-row device-cut" }, icon("hand"), cutLabel);
    const cutTip = h("span", { class: "tip device-cut-tip" }, cutButton);
    const cutCancelAction = () => {
      confirming = undefined;
      render();
      cutButton.focus();
    };
    const cutButtons = confirmButtons(t("cutAll"), () => { void cutAll(); }, cutCancelAction);
    const cutAsk = settingRow("hand", "cutConfirm", cutButtons.accept.root, cutButtons.cancel.root);
    cutAsk.root.classList.add("device-cut-confirm");
    cutAsk.root.hidden = true;
    const cut = h("div", { class: "device-cut-box" }, cutTip, cutAsk.root);
    cutButton.addEventListener("click", () => {
      confirming = "cut";
      render();
      cutButtons.cancel.button.focus();
    });

    // The warnings and the note of the socket are alerts, so they stay above the tabs whichever tab shows.
    const accessBody = tabBody(access.root, screenRow, pairingRow, cut);
    const tabs = tabset("this", t("tabListThis"), [
      { id: "access", title: "tabAccess", panel: tabColumn(accessBody) },
      { id: "who", title: "tabWho", panel: tabColumn(controllersBody) },
      {
        id: "actions", title: "sectionActions",
        panel: tabColumn(actionsBody, h("div", { class: "tab-head" }, actionSearch.root, actionsSummary, addAction.root)),
      },
      { id: "log", title: "deviceLog", panel: tabColumn(logBody, h("div", { class: "tab-head" }, logCount)) },
    ], false, () => {
      watchActions(true);
      watchLog(true);
    });
    // The log has nothing to focus, so its area takes the focus to scroll with the keys.
    logBody.scroll.tabIndex = 0;
    logBody.scroll.setAttribute("role", "region");
    logBody.scroll.setAttribute("aria-label", t("deviceLog"));
    const root = h("div", { class: "device-body" }, warningRows, socketNote.root, tabs.root);

    return {
      root,
      update(data: DevicesOverview["thisDevice"]): void {
        const remote = data.remoteAccess;
        const off = remote.remoteLogin === "off";
        const noUser = remote.userAllowed === false;
        const needsTurnOn = off || noUser || remote.passwordLogin !== "blocked";
        const note: Message = off ? "remoteNoteOff" : noUser ? "remoteNoteUser"
          : remote.passwordLogin === "allowed" ? "remoteNotePasswords"
          : remote.passwordLogin === "unknown" ? "remoteNoteUnknown" : "remoteNoteKeys";
        fadeText(accessMessage, t(note));
        const warn = noUser || remote.passwordLogin === "allowed";
        only([warnIcon, okIcon], warn ? [warnIcon] : needsTurnOn ? [] : [okIcon]);
        accessNote.toggleAttribute("data-warning", warn);
        setTip(accessInfo, t("remoteAccessHelp"));
        setAttribute(accessInfo, "aria-label", t("remoteAccessHelp"));
        fadeText(enableLabel, t(enabling ? "remoteAccessEnabling" : "remoteAccessEnable"));
        setHidden(enable, !needsTurnOn);
        if (enable.disabled !== enabling) enable.disabled = enabling;
        enable.toggleAttribute("data-attention", !enabling);
        enable.setAttribute("aria-busy", String(enabling));
        if (afterCut && (off || data.controllers.length > 0)) afterCut = false;
        setHidden(cutNote, !afterCut);
        setHidden(sharing.root, !(off || noUser || afterCut));
        const canAllow = !needsTurnOn && remote.managedFile;
        if (confirming === "passwords" && !canAllow) confirming = undefined;
        only([passwords.root, passwordConfirm],
          canAllow && confirming === "passwords" ? [passwordConfirm] : canAllow ? [passwords.root] : []);
        const screen = localScreen;
        const screenOff = screen !== undefined && !screen.on;
        const screenNoUser = screen?.on === true && screen.userAllowed === false;
        // Until the first answer the note says nothing, so it never shows a state that is not known.
        const screenKey: Message | undefined = !screen ? undefined : screenOff ? "screenNoteOff" : screenNoUser ? "screenNoteUser" : "screenNoteOn";
        fadeText(screenMessage, screenKey ? t(screenKey) : "");
        only([screenWarn, screenOk], screenNoUser ? [screenWarn] : screen?.on ? [screenOk] : []);
        screenNote.toggleAttribute("data-warning", screenNoUser);
        setTip(screenInfo, t("screenSharingHelp"));
        setAttribute(screenInfo, "aria-label", t("screenSharingHelp"));
        fadeText(screenEnableLabel, t(enablingScreen ? "screenSharingEnabling" : "screenSharingEnable"));
        setHidden(screenEnable, !screenOff);
        if (screenEnable.disabled !== enablingScreen) screenEnable.disabled = enablingScreen;
        screenEnable.toggleAttribute("data-attention", !enablingScreen);
        screenEnable.setAttribute("aria-busy", String(enablingScreen));
        setHidden(screenSettings.root, !(screenOff || screenNoUser));
        syncGuide();
        const paired = data.controllers.length;
        const pairingText = data.pairing.open && data.pairing.expires !== null
          ? fill(t("pairingWaiting"), { time: formatCountdown(data.pairing.expires - seconds()) })
          : fill(t(paired === 0 ? "pairedNone" : paired === 1 ? "pairedOne" : "pairedMany"), { count: String(paired) });
        // The countdown changes every second and only sets the text. A change of wording fades.
        const pairingKind = data.pairing.open ? "open" : `closed:${paired}`;
        if (pairingState.dataset.kind === pairingKind) setText(pairingState, pairingText);
        else fadeText(pairingState, pairingText);
        pairingState.dataset.kind = pairingKind;
        const pairingName = t(data.pairing.open ? "showCode" : "pairButton");
        fadeText(pairingButtonLabel, pairingName);
        if (pairingButton.disabled !== needsTurnOn) pairingButton.disabled = needsTurnOn;
        if (needsTurnOn) setTip(pairingTip, t("pairNeedsAccess"));
        else pairingTip.removeAttribute("data-tip");
        setAttribute(pairingButton, "aria-label", needsTurnOn ? t("pairNeedsAccess") : pairingName);
        addAction.disable(localActionsError ? t("actionsUnavailable") : undefined, t("addAction"));
        if (logDrawn !== logVersion) {
          logDrawn = logVersion;
          renderLog(logList, logCount, logBody);
        }
        if (confirming && confirming !== "cut" && confirming !== "passwords" && !confirming.startsWith("forget:") &&
          !(confirming.startsWith("action:") && localActions.some(({ id }) => `action:${id}` === confirming)) &&
          !(confirming.startsWith("shell:") && data.controllers.some(({ id, shell }) => `shell:${id}` === confirming && shell !== null)) &&
          !(confirming.startsWith("files:") && data.controllers.some(({ id, files }) => `files:${id}` === confirming && files !== null)) &&
          !(confirming.startsWith("mailbox:") && data.controllers.some(({ id, permissions }) => `mailbox:${id}` === confirming && permissions.mailbox)) &&
          !data.controllers.some(({ id }) => id === confirming)) confirming = undefined;
        warningList.sync(data.warnings.map(warning => ({ ...warning, id: String(warning.id) })));
        if (data.controllers.length === 0 && confirming === "cut") confirming = undefined;
        controllerList.sync(data.controllers);
        controllersBody.say(data.controllers.length === 0 ? t("noControllers") : "");
        const reason = data.socket.error;
        socketNote.set(fill(t("socketUnavailable"), {
          reason: reason && Object.prototype.hasOwnProperty.call(messages.en, reason) ? t(reason as Message) : reason ?? "",
        }));
        socketNote.show(!data.socket.listening);
        const matching = filterActions(localActions, actionSearch.query());
        actionList.sync(localActionsError ? [] : matching);
        fadeText(actionsSummary, localActionsError ? "" : actionCount(localActions));
        actionsBody.say(localActionsError ? errorText(localActionsError)
          : localActions.length === 0 ? t("noActions") : matching.length === 0 ? t("actionsNoMatch") : "", localActionsError !== undefined);
        const nobody = data.controllers.length === 0;
        const asking = confirming === "cut";
        if (cutButton.disabled !== nobody) cutButton.disabled = nobody;
        if (nobody) setTip(cutTip, t("noControllers"));
        else cutTip.removeAttribute("data-tip");
        only([cutTip, cutAsk.root], asking ? [cutAsk.root] : [cutTip]);
      },
    };
  }

  async function cutAll(): Promise<void> {
    confirming = undefined;
    try {
      await desktop.cutAllAccess();
      afterCut = true;
    } catch (error) {
      host.fail(error);
    }
    await refresh();
  }

  /** A code that a device sent, in words. A code that has no words shows the generic text. */
  function errorText(code: string): string {
    return Object.prototype.hasOwnProperty.call(messages.en, code) ? t(code as Message) : t("actionsUnavailable");
  }

  function outcomeText(outcome: string): string {
    const known = logOutcomes[outcome];
    if (known) return t(known);
    return Object.prototype.hasOwnProperty.call(messages.en, outcome) ? t(outcome as Message) : outcome;
  }

  /** A request or event name in words. A name that has none shows as it is. */
  function eventText(name: string): string {
    const key = `remoteLogEvent${name.charAt(0).toUpperCase()}${name.slice(1)}`;
    return Object.prototype.hasOwnProperty.call(messages.en, key) ? t(key as Message) : name;
  }

  /** What a file event touched: a path and a size, or for a grant its two choices in words. Text from outside, so only ever set as text. */
  function detailText(entry: RemoteLogEntry): string {
    if (!entry.detail) return "";
    if (entry.request === "filesGranted") {
      const [scope, mode] = entry.detail.split(" ");
      return `${t(scope === "disk" ? "filesScopeDisk" : scope === "home" ? "filesScopeHome" : "filesScopeFolders")} · ${t(mode === "readWrite" ? "filesModeWrite" : "filesModeRead")}`;
    }
    return entry.size === undefined ? entry.detail : `${entry.detail} · ${formatBytes(entry.size, host.language())}`;
  }

  /** Names and request names come from other devices, so they are only ever set as text. */
  function logItem(entry: RemoteLogEntry): HTMLElement {
    const event = eventText(entry.request);
    const detail = detailText(entry);
    return h("li", { class: "proxy-log-entry", "data-outcome": entry.outcome === "ok" ? "ok" : "refused" },
      h("time", {}, formatMoment(entry.time, host.language())),
      h("span", { class: "proxy-log-target" }, entry.name ?? "—"),
      h("span", { class: "proxy-log-target" }, event,
        entry.count && entry.count > 1
          ? h("span", { class: "log-count", "aria-label": fill(t("remoteLogRepeated"), { count: entry.count }) }, `×${entry.count}`)
          : "",
        detail !== "" && h("span", { class: "log-detail" }, detail)),
      h("span", { class: "proxy-log-outcome" }, outcomeText(entry.outcome)));
  }

  /** The entries that the list showed already, so only a new one enters with a motion. `undefined` before the first drawing. */
  let logSeen: Set<string> | undefined;
  let logLoaded = false;
  function renderLog(list: HTMLElement, count: HTMLElement, body: { say(text: string): void }): void {
    // Until the first read there is nothing to say, not even that the log is empty.
    if (!logLoaded) {
      setText(count, "");
      return;
    }
    setText(count, fill(t("remoteLogEntries"), { count: logEntries.length }));
    const seen = logSeen;
    const keys = new Set<string>();
    const items = logEntries.map(entry => {
      const item = logItem(entry);
      const key = JSON.stringify([entry.time, entry.peer, entry.request, entry.name, entry.outcome, entry.count, entry.detail, entry.size]);
      keys.add(key);
      if (seen && !seen.has(key)) window.requestAnimationFrame(() => enterNode(item));
      return item;
    });
    logSeen = keys;
    list.replaceChildren(...items);
    body.say(items.length ? "" : t("remoteLogEmpty"));
  }

  /** Reads the log while its tab shows. A failed read keeps the entries that were shown. */
  async function readLog(): Promise<void> {
    try {
      logEntries = await desktop.getRemoteLog();
    } catch (error) {
      host.fail(error);
      return;
    }
    logLoaded = true;
    logVersion++;
    render();
  }

  function logShows(): boolean {
    return !deviceList.closest("[hidden]") && overview !== undefined && showsTab("this", "log", "access");
  }

  /** Starts or stops the reading of the log. It reads at once when it starts, and when `now` says so. */
  function watchLog(now = false): void {
    if (!logShows()) {
      if (logTicker !== undefined) window.clearInterval(logTicker);
      logTicker = undefined;
      return;
    }
    const starting = logTicker === undefined;
    logTicker ??= window.setInterval(() => {
      if (logShows()) void readLog();
      else watchLog();
    }, logPoll);
    if (starting || now) void readLog();
  }

  document.addEventListener("keydown", event => {
    if (event.key !== "Escape") return;
    if (explorer.closeTop() || fileGrants.closeTop() || shells.closeTop() || windows.closeTop()) return;
    if (addOverlay.isOpen()) addOverlay.close();
    else if (pairOverlay.isOpen()) pairOverlay.close();
    else if (appsWindow.hasAttribute("data-open")) closeApps();
    else if (guideWindow.hasAttribute("data-open")) closeGuide();
    else if (confirming) {
      confirming = undefined;
      render();
    }
  });

  let guideSignature = "";
  /** True while the service of the guide still needs the user in Sharing. */
  function guideNeeded(): boolean {
    if (guideKind === "screen") return localScreen !== undefined && (!localScreen.on || localScreen.userAllowed === false);
    const access = overview?.thisDevice.remoteAccess;
    return access !== undefined && (access.remoteLogin === "off" || access.userAllowed === false);
  }

  /** Keeps an open guide true to the state, and closes it when the service is ready. */
  function syncGuide(): void {
    if (!guideWindow.hasAttribute("data-open")) return;
    if (guideNeeded()) renderGuide();
    else closeGuide(false);
  }

  /** The steps for the user to do in Sharing, when Lazy Worker could not turn a service on by itself. */
  function renderGuide(): void {
    let steps: Message[];
    let picture = false;
    let title: Message = "remoteGuideTitle";
    if (guideKind === "screen") {
      if (!localScreen) return;
      title = "screenGuideTitle";
      steps = ["screenGuideOpen"];
      if (!localScreen.on) steps.push("screenGuideSwitch");
      steps.push("screenGuideUser");
    } else {
      const access = overview?.thisDevice.remoteAccess;
      if (!access) return;
      const off = access.remoteLogin === "off";
      picture = off && access.macosMajor !== null && access.macosMajor >= 13;
      steps = ["remoteGuideOpen"];
      if (off) steps.push(access.macosMajor !== null && access.macosMajor <= 12 ? "remoteGuideTick" : "remoteGuideSwitch");
      steps.push("remoteGuideUser");
    }
    setText(guideTitle, t(title));
    const signature = JSON.stringify([host.language(), guideKind, steps, picture]);
    if (signature === guideSignature) return;
    guideSignature = signature;
    guideBody.replaceChildren(h("div", {},
      h("ol", { class: "guide-steps" }, ...steps.map(step => h("li", {}, t(step)))),
      picture && h("img", {
        class: "guide-image", src: guideImage13, alt: t("remoteGuideImageAlt"), width: "470", height: "406",
      })));
    if (guideBody.firstElementChild && guideWindow.hasAttribute("data-open")) enterNode(guideBody.firstElementChild as HTMLElement);
  }

  function openGuide(trigger: HTMLElement | undefined, kind: "login" | "screen" = "login"): void {
    guideTrigger = trigger;
    guideKind = kind;
    renderGuide();
    guideWindow.setAttribute("data-open", "");
    guideWindow.inert = false;
    guideOpen.focus();
  }

  function closeGuide(restoreFocus = true): void {
    if (!guideWindow.hasAttribute("data-open")) return;
    guideWindow.removeAttribute("data-open");
    guideWindow.inert = true;
    if (restoreFocus && guideTrigger?.isConnected) guideTrigger.focus();
  }

  guideClose.addEventListener("click", () => closeGuide());
  guideWindow.addEventListener("pointerdown", event => {
    if (event.target === guideWindow) closeGuide();
  });
  guideOpen.addEventListener("click", () => { void (guideKind === "screen" ? openScreenSettings() : openSharing()); });

  async function openSharing(): Promise<void> {
    try {
      await desktop.openSharingSettings();
    } catch (error) {
      host.fail(error);
    }
  }

  /** One administrator prompt does it all. When the system does not let it finish, the guide shows what to do by hand. */
  async function enableAccess(trigger: HTMLElement): Promise<void> {
    if (enabling) return;
    enabling = true;
    render();
    let guide = false;
    try {
      await desktop.enableRemoteAccess();
    } catch (error) {
      if (error === "remoteLoginStillOff" || error === "userStillNotAllowed") guide = true;
      // A cancelled prompt is the user's own choice, so it shows nothing.
      else if (error !== "adminCanceled") host.fail(error);
    }
    enabling = false;
    await refresh();
    if (guide) openGuide(trigger);
  }

  async function openScreenSettings(): Promise<void> {
    try {
      await desktop.openScreenSharingSettings();
    } catch (error) {
      host.fail(error);
    }
  }

  async function readLocalScreen(): Promise<void> {
    try {
      localScreen = await desktop.getScreenSharing();
    } catch {
      // The row keeps its last answer, or says nothing before the first one.
    }
    render();
  }

  /** One administrator prompt turns it on. When the system wants the switch in Settings, the guide shows what to do by hand. */
  async function enableScreen(trigger: HTMLElement): Promise<void> {
    if (enablingScreen) return;
    enablingScreen = true;
    render();
    let guide = false;
    try {
      localScreen = await desktop.enableScreenSharing();
    } catch (error) {
      if (error === "screenStillOff" || error === "userStillNotAllowed") guide = true;
      // A cancelled prompt is the user's own choice, so it shows nothing.
      else if (error !== "adminCanceled") host.fail(error);
    }
    enablingScreen = false;
    await readLocalScreen();
    if (guide) openGuide(trigger, "screen");
  }

  async function allowPasswords(): Promise<void> {
    confirming = undefined;
    try {
      await desktop.allowPasswordLogin();
    } catch (error) {
      if (error !== "adminCanceled") host.fail(error);
    }
    await refresh();
  }

  /** A line of a window: an icon, a label that follows the language, and the control at the end. */
  function overlayRow(name: string, label: Message, tag: "div" | "label" = "div", ...end: Child[]) {
    const text = h("span", { class: "setting-label" }, bind(h("span"), label));
    return { text, root: h(tag, { class: "setting-row" }, icon(name), text, h("div", { class: "device-end" }, ...end)) };
  }

  /** A text field that keeps only what its alphabet allows as the user types, and the caret where it was. */
  function filtered(input: HTMLInputElement, filter: (text: string) => string, changed: () => void): void {
    input.addEventListener("input", () => {
      const start = input.selectionStart ?? input.value.length;
      const next = filter(input.value);
      if (next !== input.value) {
        const caret = filter(input.value.slice(0, start)).length;
        input.value = next;
        input.setSelectionRange(caret, caret);
      }
      changed();
    });
  }

  /** The window that opens over the view. It restores the focus to what opened it. */
  function overlay(shell: HTMLElement, closeButton: HTMLElement, onClose: () => void) {
    let trigger: HTMLElement | undefined;
    const api = {
      isOpen: () => shell.hasAttribute("data-open"),
      open(from: HTMLElement | undefined, focus: HTMLElement): void {
        trigger = from;
        shell.setAttribute("data-open", "");
        shell.inert = false;
        focus.focus();
      },
      close(restoreFocus = true): void {
        if (!api.isOpen()) return;
        shell.removeAttribute("data-open");
        shell.inert = true;
        onClose();
        if (restoreFocus && trigger?.isConnected) trigger.focus();
      },
    };
    closeButton.addEventListener("click", () => api.close());
    shell.addEventListener("pointerdown", event => {
      if (event.target === shell) api.close();
    });
    return api;
  }

  // ---- Add device: choose a device on this network or type its address, then pair with its code ----
  const addWindow = document.getElementById("add-device-window")!;
  const addBody = document.getElementById("add-device-body")!;
  const addBack = document.getElementById("add-device-back") as HTMLButtonElement;
  const addClose = document.getElementById("add-device-close") as HTMLButtonElement;
  type Found = DiscoveredDevice & { id: string };
  let found: Found[] = [];
  let scanning = false;
  let scanError: Message | undefined;
  let scanSequence = 0;
  type Chosen = { name: string; host: string; port: number; pairing: DiscoveredDevice["pairing"] };
  let selected: Chosen | undefined;
  let step: "choose" | "pair" = "choose";
  /** The search that repeats while the list shows, so a pairing window that opens later is found. */
  let rescanTimer: number | undefined;
  const rescanEvery = 3000;
  let pairBusy = false;
  let pairLocked = false;
  let pairErrorKey: Message | undefined;
  /** Changes when the window closes or goes back, so the answer of an old pairing does not touch the new screen. */
  let addAttempt = 0;

  const rescan = iconButton(t("addSearchAgain"), "refresh-cw", "outlined");
  bindTip(rescan, "addSearchAgain");
  const foundNote = note("");
  const foundRows = h("div", { class: "device-rows" });
  const foundSignature = (device: Found): string => JSON.stringify([host.language(), device.name, device.host, device.port]);
  const foundList = list<Found>(foundRows, foundSignature, foundRow);
  // A device without an open pairing window is only an SSH server on the network. It waits behind a count,
  // so the list shows first the devices that want to pair.
  let showOthers = false;
  const otherRows = h("div", { class: "device-rows found-others-rows" });
  const otherList = list<Found>(otherRows, foundSignature, otherRow);
  const othersLabel = h("span");
  const othersToggle = h("button", {
    type: "button", class: "device-row found-row found-others disclosure", "aria-expanded": "false", "aria-controls": "found-others",
  }, icon("eye-off"), othersLabel, icon("chevron-right"));
  const othersToggleRow = h("div", { class: "collapse", inert: true }, h("div", {}, othersToggle));
  const others = h("div", { class: "collapse", id: "found-others", inert: true }, h("div", {}, otherRows));
  othersToggle.addEventListener("click", () => {
    showOthers = !showOthers;
    renderFound();
  });
  const addressField = h("input", {
    class: "field", type: "text", autocomplete: "off", autocapitalize: "off", spellcheck: "false", inputmode: "url",
  });
  bind(addressField, "addAddressPlaceholder", "placeholder");
  bind(addressField, "addAddressLabel", "aria-label");
  const addressNext = iconButton(t("continue"), "arrow-right", "confirm");
  bindTip(addressNext, "continue");
  addressNext.button.type = "submit";
  const addressError = note("", true);
  const addressForm = h("form", { class: "pair-form", novalidate: true },
    h("div", { class: "setting-row" }, icon("network"), addressField, h("div", { class: "device-end" }, addressNext.root)),
    addressError.root,
    bind(h("p", { class: "setting-note pair-hint" }), "addAddressHint"));
  const chooseStep = h("div", { class: "pair-step" },
    h("div", { class: "device-subhead" }, bind(h("h3"), "addFound"), rescan.root),
    foundNote.root, foundRows, othersToggleRow, others,
    h("div", { class: "device-subhead" }, bind(h("h3"), "addEnterAddress")), addressForm);

  const summaryName = h("strong");
  const summaryWhere = h("span", { class: "setting-note" });
  const accountField = h("input", {
    class: "field pair-field", type: "text", autocomplete: "username", autocapitalize: "off", spellcheck: "false", maxlength: "32",
  });
  const accountInfo = h("span", { class: "tip option-info", tabindex: "0", role: "img" }, icon("info"));
  // The account that an open pairing window announced is shown as text, with a lock, and cannot be changed.
  const accountFixedName = h("span", { class: "pair-account-name" });
  const accountFixed = h("span", { class: "tip pair-account-fixed", tabindex: "0", hidden: true },
    accountFixedName, icon("lock-keyhole"));
  bind(accountFixed, "addAccountShown", "data-tip");
  bind(accountInfo, "addAccountHelp", "data-tip");
  bind(accountInfo, "addAccountHelp", "aria-label");
  bind(accountField, "addAccountPlaceholder", "placeholder");
  bind(accountField, "addAccount", "aria-label");
  const account = overlayRow("user-round", "addAccount", "label", accountField, accountFixed);
  account.text.append(" ", accountInfo);
  const accountError = note("", true);
  const codeField = h("input", {
    class: "field pair-code-field", type: "text", inputmode: "numeric", autocomplete: "one-time-code", spellcheck: "false",
    placeholder: "000 000", "aria-describedby": "pair-code-hint",
  });
  const code = overlayRow("key-round", "addCode", "label", codeField);
  const pairLabel = h("span");
  const pairButton = h("button", { type: "submit", class: "access-action" }, icon("link"), pairLabel);
  const pairError = note("", true);
  pairError.root.setAttribute("aria-live", "polite");
  const pairStep = h("form", { class: "pair-step pair-form", novalidate: true },
    h("div", { class: "device-row" }, icon("monitor"), h("div", { class: "device-row-text" }, summaryName, summaryWhere)),
    account.root, accountError.root, code.root,
    bind(h("p", { class: "setting-note pair-hint", id: "pair-code-hint" }), "addCodeHint"),
    pairError.root,
    h("div", { class: "guide-actions" }, pairButton));
  addBody.append(chooseStep, pairStep);

  const addAccountValid = (text: string): boolean => /^[A-Za-z0-9_][A-Za-z0-9_.-]{0,31}$/.test(text);
  const accountFilter = (text: string): string => text.replace(/[^A-Za-z0-9_.-]/g, "").slice(0, 32);
  const addressFilter = (text: string): string => text.replace(/[^A-Za-z0-9.:\-[\]%]/g, "").slice(0, 80);
  const digitsOf = (text: string): string => text.replace(/\D/g, "").slice(0, 6);
  /** The code in two groups of three, as the other device shows it. */
  const codeFilter = (text: string): string => {
    const digits = digitsOf(text);
    return digits.length > 3 ? `${digits.slice(0, 3)} ${digits.slice(3)}` : digits;
  };

  function foundRow(device: Found): Row<Found> {
    let latest = device;
    const where = device.port === 22 ? device.host : `${device.host}:${device.port}`;
    // The badge fades in and out beside the chevron when the device opens or closes its pairing window,
    // so the row keeps its element and its height.
    const open = h("span", { class: "setting-note found-pairing", "aria-hidden": "true" }, icon("radio"), t("addPairingOpen"));
    const root = h("button", { type: "button", class: "device-row found-row" },
      kindIcon("desktop"),
      h("div", { class: "device-row-text" }, oneLine(h("strong", {}, device.name)), h("span", { class: "setting-note" }, where)),
      open,
      icon("chevron-right"));
    root.addEventListener("click", () => choose({
      name: latest.name, host: latest.host, port: latest.port, pairing: latest.pairing,
    }));
    return {
      root,
      update(next) {
        latest = next;
        const announced = next.pairing !== null;
        open.toggleAttribute("data-open", announced);
        open.setAttribute("aria-hidden", String(!announced));
        root.toggleAttribute("data-pairing", announced);
      },
    };
  }

  /** A device without an open pairing window cannot pair, so its row only says that it is there. */
  function otherRow(device: Found): Row<Found> {
    const where = device.port === 22 ? device.host : `${device.host}:${device.port}`;
    const root = h("div", { class: "device-row found-info" },
      kindIcon("desktop"),
      h("div", { class: "device-row-text" }, oneLine(h("strong", {}, device.name)), h("span", { class: "setting-note" }, where)));
    return { root, update() {} };
  }

  function settleIcons(): void {
    if (built) {
      built = false;
      host.refreshIcons();
    }
  }

  function renderFound(): void {
    rescan.disable(scanning ? t("addSearching") : undefined, t("addSearchAgain"));
    rescan.button.toggleAttribute("aria-busy", scanning);
    const pairing = found.filter(device => device.pairing !== null);
    const rest = found.filter(device => device.pairing === null);
    foundList.sync(pairing);
    otherList.sync(rest);
    if (!rest.length) showOthers = false;
    othersToggleRow.toggleAttribute("data-open", rest.length > 0);
    othersToggleRow.inert = rest.length === 0;
    others.toggleAttribute("data-open", showOthers);
    others.inert = !showOthers;
    othersToggle.setAttribute("aria-expanded", String(showOthers));
    const othersText = showOthers ? t("addOthersHide")
      : rest.length === 1 ? t("addOthersOne") : fill(t("addOthersMany"), { count: String(rest.length) });
    if (othersLabel.textContent !== othersText) fadeText(othersLabel, othersText);
    const text = scanning && !found.length ? t("addSearching")
      : scanError ? t(scanError) : !found.length ? t("addNoneFound") : pairing.length ? "" : t("addNonePairing");
    if (text) foundNote.set(text);
    foundNote.show(text !== "");
    settleIcons();
  }

  /** Searches the network. A quiet search is the repeat of the list: it shows no progress, and it keeps the list when it fails. */
  async function scan(quiet = false): Promise<void> {
    window.clearTimeout(rescanTimer);
    const current = ++scanSequence;
    if (!quiet) {
      scanning = true;
      renderFound();
    }
    try {
      const result = await desktop.discoverDevices();
      if (current !== scanSequence) return;
      found = result.map(device => ({ ...device, id: `${device.host}:${device.port}` }));
      scanError = undefined;
    } catch {
      if (current !== scanSequence) return;
      if (!quiet) {
        found = [];
        scanError = "discoveryFailed";
      }
    }
    scanning = false;
    renderFound();
    // Repeat while the list shows, so a window that opens after this one is found.
    rescanTimer = window.setTimeout(() => {
      if (addOverlay.isOpen() && step === "choose") void scan(true);
    }, rescanEvery);
  }

  /** A name or an address, with an optional port. The app checks it again before it connects. */
  function parseAddress(text: string): { host: string; port: number | null } | "invalid" | undefined {
    const value = text.trim();
    if (!value) return undefined;
    const port = (digits: string): number | undefined =>
      /^\d{1,5}$/.test(digits) && Number(digits) >= 1 && Number(digits) <= 65535 ? Number(digits) : undefined;
    const v6 = (address: string): boolean =>
      /^[0-9A-Fa-f:.]{2,45}(%[A-Za-z0-9]{1,15})?$/.test(address) && address.split(":").length > 2;
    if (value.startsWith("[")) {
      const end = value.indexOf("]");
      if (end < 0 || !v6(value.slice(1, end))) return "invalid";
      const rest = value.slice(end + 1);
      if (!rest) return { host: value.slice(1, end), port: null };
      const number = rest.startsWith(":") ? port(rest.slice(1)) : undefined;
      return number === undefined ? "invalid" : { host: value.slice(1, end), port: number };
    }
    const colons = value.split(":").length - 1;
    if (colons > 1) return v6(value) ? { host: value, port: null } : "invalid";
    const [name, digits] = value.split(":");
    const number = colons === 1 ? port(digits) : null;
    if (number === undefined) return "invalid";
    const labels = name.split(".");
    const ipv4 = /^\d{1,3}(\.\d{1,3}){3}$/.test(name) && labels.every(label => Number(label) <= 255);
    const dns = name.length <= 253 && !/^\d+$/.test(labels[labels.length - 1]) &&
      labels.every(label => /^[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?$/.test(label));
    return ipv4 || dns ? { host: name, port: number } : "invalid";
  }

  function renderAddress(): void {
    const parsed = parseAddress(addressField.value);
    const bad = parsed === "invalid";
    addressField.toggleAttribute("aria-invalid", bad);
    if (bad) addressError.set(t("addressInvalid"));
    addressError.show(bad);
    addressNext.disable(parsed === undefined || bad ? t(bad ? "addressInvalid" : "continue") : undefined, t("continue"));
  }

  function renderPairForm(): void {
    const user = accountField.value;
    const bad = user !== "" && !addAccountValid(user);
    accountField.toggleAttribute("aria-invalid", bad);
    if (bad) accountError.set(t("addAccountInvalid"));
    accountError.show(bad);
    if (pairErrorKey) pairError.set(t(pairErrorKey));
    pairError.show(pairErrorKey !== undefined);
    fadeText(pairLabel, t(pairBusy ? "addPairing" : "addPair"));
    const ready = addAccountValid(user) && digitsOf(codeField.value).length === 6 && !pairBusy && !pairLocked;
    if (pairButton.disabled !== !ready) pairButton.disabled = !ready;
    pairButton.setAttribute("aria-busy", String(pairBusy));
    for (const field of [accountField, codeField]) field.readOnly = pairBusy;
    const fixed = selected?.pairing?.account;
    if (fixed !== undefined) {
      setText(accountFixedName, fixed);
      setAttribute(accountFixed, "aria-label", `${t("addAccount")}: ${fixed}. ${t("addAccountShown")}`);
    }
    accountField.hidden = fixed !== undefined;
    accountFixed.hidden = fixed === undefined;
    accountInfo.hidden = fixed !== undefined;
    setText(summaryName, selected?.name ?? "");
    setText(summaryWhere, selected ? (selected.port === 22 ? selected.host : `${selected.host}:${selected.port}`) : "");
  }

  function showStep(next: "choose" | "pair"): void {
    step = next;
    only([chooseStep, pairStep], [next === "choose" ? chooseStep : pairStep]);
    setHidden(addBack, next === "choose");
    renderPairForm();
  }

  function choose(device: Chosen): void {
    // An answer of the search that is still running must not change the list under this step.
    scanSequence += 1;
    scanning = false;
    window.clearTimeout(rescanTimer);
    selected = device;
    pairLocked = false;
    pairErrorKey = undefined;
    codeField.value = "";
    // The account of an open pairing window fills the field. Without one, the field is empty and the user types it.
    accountField.value = device.pairing?.account ?? "";
    showStep("pair");
    (accountField.value ? codeField : accountField).focus();
  }

  function goBack(): void {
    addAttempt += 1;
    pairBusy = false;
    selected = undefined;
    codeField.value = "";
    pairErrorKey = undefined;
    accountField.value = "";
    showStep("choose");
    addressField.focus();
    void scan(true);
  }

  const addOverlay = overlay(addWindow, addClose, () => {
    scanSequence += 1;
    window.clearTimeout(rescanTimer);
    scanning = false;
    addAttempt += 1;
    pairBusy = false;
    selected = undefined;
    addressField.value = "";
    accountField.value = "";
    codeField.value = "";
    pairErrorKey = undefined;
    showStep("choose");
    renderAddress();
  });

  function openAdd(trigger: HTMLElement): void {
    found = [];
    showOthers = false;
    scanError = undefined;
    showStep("choose");
    renderAddress();
    addOverlay.open(trigger, addressField);
    void scan();
  }

  async function submitPair(): Promise<void> {
    if (!selected || pairBusy) return;
    const target = selected;
    const attempt = addAttempt;
    pairBusy = true;
    pairErrorKey = undefined;
    renderPairForm();
    try {
      const device = await desktop.pairDevice(accountField.value, target.host, target.port === 22 ? null : target.port, digitsOf(codeField.value));
      // The new device opens its row when it appears.
      setOpen(`r:${device.id}`, true);
      pairBusy = false;
      if (attempt === addAttempt) addOverlay.close(false);
      await refresh();
      return;
    } catch (error) {
      pairBusy = false;
      if (attempt !== addAttempt) return;
      pairErrorKey = typeof error === "string" && Object.prototype.hasOwnProperty.call(pairErrors, error) ? pairErrors[error] : "pairGeneric";
      // A device that cannot prove who it is ends the attempt here. Going back is the way to start again.
      pairLocked = typeof error === "string" && stopErrors.has(error);
      renderPairForm();
      if (error === "wrongCode") {
        codeField.focus();
        codeField.select();
      }
    }
  }

  addBack.addEventListener("click", goBack);
  rescan.button.addEventListener("click", () => { void scan(); });
  addressForm.addEventListener("submit", event => {
    event.preventDefault();
    const parsed = parseAddress(addressField.value);
    if (parsed && parsed !== "invalid") choose({ name: parsed.host, host: parsed.host, port: parsed.port ?? 22, pairing: null });
  });
  pairStep.addEventListener("submit", event => {
    event.preventDefault();
    void submitPair();
  });
  filtered(addressField, addressFilter, renderAddress);
  filtered(accountField, accountFilter, () => { pairLocked = false; renderPairForm(); });
  filtered(codeField, codeFilter, () => { pairLocked = false; renderPairForm(); });

  // ---- Pair a device: this device shows a code for ten minutes ----
  const pairWindow = document.getElementById("pair-device-window")!;
  const pairBody = document.getElementById("pair-device-body")!;
  const pairClose = document.getElementById("pair-device-close") as HTMLButtonElement;
  type Stage = "setup" | "open" | "done";
  let stage: Stage = "setup";
  let shown: { code: string; account: string; expires: number } | undefined;
  let baseline = new Set<string>();
  let outcomeName: string | undefined;
  let opening = false;
  let openFailed = false;
  let reach: ProxyNetwork | undefined;
  const grant = { control: true, applications: false, actions: false, screen: false };

  function permissionSwitch(id: keyof typeof grant, iconName: string, label: Message, hint: Message) {
    const input = h("input", { type: "checkbox", switch: true, role: "switch" });
    input.checked = grant[id];
    input.addEventListener("change", () => { grant[id] = input.checked; });
    const row = overlayRow(iconName, label, "label", h("span", { class: "switch" }, input, h("span", { "aria-hidden": "true" })));
    // The sentence sits under the switch, so the choice is made with it in view.
    const sentence = bind(h("p", { class: "setting-note pair-permission-hint" }), hint);
    input.setAttribute("aria-describedby", `pair-permission-${id}`);
    sentence.id = `pair-permission-${id}`;
    return { root: h("div", { class: "pair-permission" }, row.root, sentence), input };
  }
  const switches = [
    permissionSwitch("control", "layout-grid", "permControl", "permControlHint"),
    permissionSwitch("applications", "app-window", "permApplications", "permApplicationsHint"),
    permissionSwitch("actions", "play", "permActions", "permActionsHint"),
    permissionSwitch("screen", "monitor", "permScreen", "permScreenHint"),
  ];
  const openLabel = h("span");
  const openButton = h("button", { type: "button", class: "access-action" }, icon("lock-keyhole-open"), openLabel);
  const openError = note("", true);
  const setupStage = h("div", { class: "pair-step" },
    h("div", { class: "device-subhead" }, bind(h("h3"), "pairPermissionsTitle")),
    ...switches.map(({ root }) => root),
    bind(h("p", { class: "setting-note pair-hint" }), "pairShellNever"),
    openError.root,
    h("div", { class: "guide-actions" }, openButton));

  const codeText = h("output", { class: "pair-code" });
  const codeHidden = bind(h("p", { class: "setting-note" }), "pairCodeHidden");
  const accountText = h("dd");
  const reachText = h("dd", { class: "pair-reach" });
  const timeText = h("span", { class: "pair-time" });
  const attemptsText = h("dd");
  const closeLabel = h("span");
  const closePairingButton = h("button", { type: "button", class: "access-action" }, icon("hand"), closeLabel);
  const fact = (label: Message, ...value: Child[]) => [bind(h("dt"), label), ...value];
  const openStage = h("div", { class: "pair-step" },
    h("div", { class: "pair-code-box" },
      bind(h("p", { class: "setting-note" }), "pairCodeLabel"), codeText, codeHidden,
      bind(h("p", { class: "setting-note" }), "pairCodeCopyHint")),
    h("dl", { class: "pair-facts" },
      ...fact("pairAccountLabel", accountText), ...fact("pairReachLabel", reachText),
      ...fact("pairTimeLeft", h("dd", {}, timeText)), ...fact("pairAttemptsLeft", attemptsText)),
    h("div", { class: "guide-actions" }, closePairingButton));

  const resultIcon = h("span", { class: "pair-result-icon" }, icon("check"));
  const resultText = h("p", { class: "pair-result-text", role: "status" });
  const doneButton = h("button", { type: "button", class: "access-action" }, icon("check"), bind(h("span"), "pairDone"));
  const doneStage = h("div", { class: "pair-step pair-result" }, resultIcon, resultText, h("div", { class: "guide-actions" }, doneButton));
  pairBody.append(setupStage, openStage, doneStage);

  const pairOverlay = overlay(pairWindow, pairClose, () => {
    stage = "setup";
    outcomeName = undefined;
    openFailed = false;
    Object.assign(grant, { control: true, applications: false, actions: false, screen: false });
    switches.forEach(({ input }, index) => { input.checked = index === 0; });
    renderPairWindow();
  });

  function ids(): Set<string> {
    return new Set((overview?.thisDevice.controllers ?? []).map(({ id }) => id));
  }

  function renderPairWindow(): void {
    const state = overview?.thisDevice.pairing;
    if (state) {
      if (stage === "setup" && state.open && pairOverlay.isOpen()) stage = "open";
      if (stage === "open" && !state.open && !opening) {
        // The window ended: its time ran out, the code was entered, or the attempts ran out.
        const added = (overview?.thisDevice.controllers ?? []).filter(({ id }) => !baseline.has(id));
        outcomeName = added.length ? added[added.length - 1].name : undefined;
        stage = "done";
      }
    }
    only([setupStage, openStage, doneStage], [stage === "setup" ? setupStage : stage === "open" ? openStage : doneStage]);
    fadeText(openLabel, t(opening ? "pairOpening" : "pairOpenFor"));
    if (openButton.disabled !== opening) openButton.disabled = opening;
    openButton.setAttribute("aria-busy", String(opening));
    if (openFailed) openError.set(t("pairOpenFailed"));
    openError.show(openFailed);
    setText(closeLabel, t("pairClosePairing"));
    if (state && stage === "open") {
      const current = shown && shown.expires === state.expires ? shown : undefined;
      setText(codeText, current ? codeFilter(current.code) : "");
      only([codeText, codeHidden], [current ? codeText : codeHidden]);
      setText(accountText, current?.account ?? overview?.thisDevice.account ?? "—");
      const names = [reach?.hostname, ...(reach?.addresses ?? [])].filter((name): name is string => Boolean(name));
      reachText.replaceChildren(...(names.length ? names.map(name => h("span", {}, name)) : [h("span", {}, "—")]));
      setText(timeText, formatSession((state.expires ?? 0) - seconds()));
      setText(attemptsText, String(state.attemptsLeft));
    }
    setHidden(resultIcon, outcomeName === undefined);
    fadeText(resultText, outcomeName === undefined ? t("pairEnded") : fill(t("pairPaired"), { name: outcomeName }));
    settleIcons();
  }

  function openPairWindow(trigger: HTMLElement): void {
    stage = overview?.thisDevice.pairing.open ? "open" : "setup";
    baseline = ids();
    openFailed = false;
    reach = undefined;
    void desktop.getProxyNetwork().then(network => {
      reach = network;
      renderPairWindow();
    }, () => undefined);
    renderPairWindow();
    pairOverlay.open(trigger, stage === "open" ? closePairingButton : openButton);
    syncTicker();
  }

  async function startPairing(): Promise<void> {
    if (opening) return;
    opening = true;
    openFailed = false;
    baseline = ids();
    renderPairWindow();
    try {
      shown = await desktop.openPairing({ ...grant });
      stage = "open";
    } catch {
      openFailed = true;
    }
    await refresh();
    opening = false;
    renderPairWindow();
    if (stage === "open") closePairingButton.focus();
  }

  async function stopPairing(): Promise<void> {
    try {
      await desktop.closePairing();
    } catch (error) {
      host.fail(error);
    }
    outcomeName = undefined;
    stage = "done";
    await refresh();
    renderPairWindow();
    doneButton.focus();
  }

  openButton.addEventListener("click", () => { void startPairing(); });
  closePairingButton.addEventListener("click", () => { void stopPairing(); });
  doneButton.addEventListener("click", () => pairOverlay.close());

  /** While a pairing window is open, the view asks the app once a second, so the countdown and the outcome stay true. */
  let ticker: number | undefined;
  function syncTicker(): void {
    const visible = !deviceList.closest("[hidden]");
    const needed = visible && (pairOverlay.isOpen() || overview?.thisDevice.pairing.open === true);
    if (needed && ticker === undefined) {
      ticker = window.setInterval(() => {
        if (deviceList.closest("[hidden]")) syncTicker();
        else void refresh();
      }, 1000);
    } else if (!needed && ticker !== undefined) {
      window.clearInterval(ticker);
      ticker = undefined;
    }
  }

  /** Puts the words of the windows in the language of the interface. */
  function retranslate(): void {
    for (const apply of labelled) apply();
    renderFound();
    renderAddress();
    renderPairForm();
    renderPairWindow();
    windows.retranslate();
    shells.retranslate();
    fileGrants.retranslate();
    explorer.retranslate();
  }

  /** Leaving the view drops what waits for a second click, closes the windows, and drops what was typed in them. */
  function reset(): void {
    const cleared = [...searches].map(clear => clear()).includes(true);
    const wasConfirming = confirming !== undefined;
    closeApps(false);
    closeGuide(false);
    addOverlay.close(false);
    pairOverlay.close(false);
    windows.closeAll();
    shells.closeAll();
    fileGrants.closeAll();
    explorer.closeAll();
    syncTicker();
    watchTools();
    watchApps();
    watchActions();
    watchSessions();
    watchLog();
    watchReach();
    confirming = undefined;
    if (cleared || wasConfirming) render();
  }

  /** The tools of a device as the Devices view lists them, from the statuses that it sent. Tools it cannot run are left out. */
  function listTools(statuses: Statuses): RemoteTool[] {
    return tools.filter(({ id }) => statuses[id]?.supported).map(({ id }) => ({ id, running: statuses[id].mode !== "off" }));
  }

  /** Reads the tools of one device. A device that has a read in progress is skipped, so a slow link never piles requests up. */
  async function readTools(id: string): Promise<void> {
    if (polling.has(id)) return;
    polling.add(id);
    try {
      const entry = live.get(id);
      const found = listTools(await desktop.remoteStatuses(id));
      live.set(id, { ...entry, tools: found, error: undefined });
    } catch (error) {
      live.set(id, { ...live.get(id), error: typeof error === "string" ? error : "remoteFailed" });
    } finally {
      polling.delete(id);
    }
    render();
  }

  /** Reads what a device allows and whether its shell is on. A device that has a read in progress is skipped. */
  async function readSession(id: string): Promise<void> {
    if (readingSessions.has(id)) return;
    readingSessions.add(id);
    try {
      const session = await desktop.remoteSession(id);
      live.set(id, { ...live.get(id), permissions: session.permissions, shell: session.shell });
    } catch {
      // The tools and the tabs say that the link failed. The last answer stays.
    } finally {
      readingSessions.delete(id);
    }
    render();
  }

  /** Reads whether Screen Sharing is on at a device. A device that did not allow Control is not asked. */
  async function readScreen(id: string): Promise<void> {
    if (readingScreens.has(id)) return;
    const denied = refusal("getScreenSharing", undefined, live.get(id)?.permissions);
    if (denied) {
      if (live.get(id)?.screenError !== denied) live.set(id, { ...live.get(id), screen: undefined, screenError: denied });
      render();
      return;
    }
    readingScreens.add(id);
    try {
      live.set(id, { ...live.get(id), screen: await desktop.remoteScreenSharing(id), screenError: undefined });
    } catch (error) {
      live.set(id, { ...live.get(id), screenError: typeof error === "string" ? error : "remoteFailed" });
    } finally {
      readingScreens.delete(id);
    }
    render();
  }

  /** Reads what a device shares with this one. Any paired device may ask. */
  async function readFiles(id: string): Promise<void> {
    if (readingFiles.has(id)) return;
    readingFiles.add(id);
    try {
      live.set(id, { ...live.get(id), files: await desktop.remoteFileShare(id), filesError: undefined });
    } catch (error) {
      live.set(id, { ...live.get(id), filesError: typeof error === "string" ? error : "remoteFailed" });
    } finally {
      readingFiles.delete(id);
    }
    render();
  }

  /** The devices whose row is open, whichever tab shows. Their session is what the tabs and the shell button show. */
  function watchedSessionDevices(): RemoteDevice[] {
    if (deviceList.closest("[hidden]") || !overview) return [];
    return overview.devices.filter(device => device.online !== false && isOpen(`r:${device.id}`));
  }

  /** Starts or stops the reading of sessions. It reads at once when it starts, and when `now` says so. */
  function watchSessions(now = false): void {
    const wanted = watchedSessionDevices();
    if (wanted.length === 0) {
      if (sessionTicker !== undefined) window.clearInterval(sessionTicker);
      sessionTicker = undefined;
      return;
    }
    const starting = sessionTicker === undefined;
    sessionTicker ??= window.setInterval(() => {
      const current = watchedSessionDevices();
      if (current.length === 0) watchSessions();
      else for (const device of current) void readSession(device.id).then(() => readScreen(device.id)).then(() => readFiles(device.id));
    }, sessionPoll);
    if (starting || now) for (const device of wanted) void readSession(device.id).then(() => readScreen(device.id)).then(() => readFiles(device.id));
  }

  /** The devices whose tools the tab bar shows now: the view and the device row are open. Every tab says something in its title. */
  function watchedDevices(): RemoteDevice[] {
    return watchedSessionDevices();
  }

  /** Starts or stops the reading of tools. It reads at once when it starts, and when `now` says so. */
  function watchTools(now = false): void {
    const wanted = watchedDevices();
    if (wanted.length === 0) {
      if (toolTicker !== undefined) window.clearInterval(toolTicker);
      toolTicker = undefined;
      return;
    }
    const starting = toolTicker === undefined;
    toolTicker ??= window.setInterval(() => {
      const current = watchedDevices();
      if (current.length === 0) watchTools();
      else for (const device of current) void readTools(device.id);
    }, toolPoll);
    if (starting || now) for (const device of wanted) void readTools(device.id);
  }

  /** Turns a tool of another device on or off. The device decides, and a refusal is shown in the section. */
  async function changeTool(device: { id: string }, tool: RemoteTool): Promise<void> {
    try {
      const found = listTools(await desktop.remoteSetToolActive(device.id, tool.id, !tool.running));
      live.set(device.id, { ...live.get(device.id), tools: found, error: undefined });
    } catch (error) {
      if (isRefusal(error) || isLinkError(error)) live.set(device.id, { ...live.get(device.id), error });
      else host.fail(error);
    }
    render();
  }

  /** Why a tool of another device cannot be turned on or off from here, in the words of the interface. */
  function toolReason(tool: RemoteTool, name: string, entry: ReturnType<typeof live.get>): string | undefined {
    if (entry?.error && isLinkError(entry.error)) return t("remoteLost");
    const reason = refusal("setToolActive", { tool: tool.id, active: !tool.running }, entry?.permissions);
    return reason && fill(t(reason), { name });
  }

  function toolRow(device: { id: string; name: string }, tool: RemoteTool): Row<RemoteTool> {
    const definition = tools.find(({ id }) => id === tool.id)!;
    const name = t(definition.title);
    const status = h("span", { class: "status" });
    const power = h("button", {
      type: "button", class: "power catalog-power", role: "switch", "aria-label": name, "aria-checked": "false",
    }, icon("power"));
    const tip = h("span", { class: "tip" }, power);
    const root = h("div", { class: "device-row" },
      h("span", { class: "tool-name" }, icon(definition.icon), h("strong", {}, name)), status, tip);
    root.querySelector(".tool-name > i")!.classList.add("tool-icon");
    let latest = tool;
    power.addEventListener("click", () => void changeTool(device, latest));
    return {
      root,
      update(current) {
        latest = current;
        status.dataset.mode = current.running ? "manual" : "off";
        fadeText(status, t(current.running ? "manual" : "off"));
        setAttribute(power, "aria-checked", String(current.running));
        const reason = toolReason(current, device.name, live.get(device.id));
        if (power.disabled !== (reason !== undefined)) power.disabled = reason !== undefined;
        setTip(tip, reason ?? fill(t(current.running ? "toolPowerOff" : "toolPowerOn"), { tool: name }));
      },
    };
  }

  function appsOf(id: string): AppsState {
    let state = appsStates.get(id);
    if (!state) {
      state = { opening: new Set() };
      appsStates.set(id, state);
    }
    return state;
  }

  /** Reads the applications of one device. A device that has a read in progress is skipped. */
  async function readApps(id: string): Promise<void> {
    if (readingApps.has(id)) return;
    readingApps.add(id);
    const state = appsOf(id);
    try {
      // The device's own answer says whether it allowed applications, so a refused request is not sent again and again.
      if (state.permitted !== true) state.permitted = (await desktop.remoteSession(id)).permissions.applications === true;
      if (state.permitted) {
        state.apps = await desktop.remoteApplications(id);
        state.error = undefined;
      } else {
        state.apps = undefined;
        state.error = undefined;
      }
    } catch (error) {
      if (error === "notPermitted") {
        state.permitted = false;
        state.apps = undefined;
        state.error = undefined;
      } else {
        state.error = typeof error === "string" ? error : "remoteFailed";
      }
    } finally {
      readingApps.delete(id);
    }
    render();
  }

  /** The devices whose applications the user can see now: the device is open, or the window of the list is. */
  function watchedAppDevices(): RemoteDevice[] {
    if (!overview) return [];
    const viewing = !deviceList.closest("[hidden]");
    return overview.devices.filter(device => device.online !== false && (device.id === appsDevice
      || (viewing && isOpen(`r:${device.id}`))));
  }

  /** Starts or stops the reading of applications. It reads at once when it starts, and when `now` says so. */
  function watchApps(now = false): void {
    const wanted = watchedAppDevices();
    if (wanted.length === 0) {
      if (appTicker !== undefined) window.clearInterval(appTicker);
      appTicker = undefined;
      return;
    }
    const starting = appTicker === undefined;
    appTicker ??= window.setInterval(() => {
      const current = watchedAppDevices();
      if (current.length === 0) watchApps();
      else for (const device of current) void readApps(device.id);
    }, appPoll);
    if (starting || now) for (const device of wanted) void readApps(device.id);
  }

  function actionsOf(id: string): ActionsState {
    let state = actionStates.get(id);
    if (!state) {
      state = {};
      actionStates.set(id, state);
    }
    return state;
  }

  /** Reads the actions of this device. */
  async function readLocalActions(): Promise<void> {
    try {
      localActions = await desktop.listActions();
      localActionsError = undefined;
    } catch (error) {
      localActionsError = typeof error === "string" ? error : "actionsUnavailable";
    }
    render();
  }

  /** Reads the actions of one device. A device that has a read in progress is skipped. */
  async function readActions(id: string): Promise<void> {
    if (readingActions.has(id)) return;
    readingActions.add(id);
    const state = actionsOf(id);
    try {
      // The device's own answer says whether it allowed actions, so a refused request is not sent again and again.
      if (state.permitted !== true) state.permitted = (await desktop.remoteSession(id)).permissions.actions === true;
      if (state.permitted) {
        state.actions = await desktop.remoteListActions(id);
        state.error = undefined;
      } else {
        state.actions = undefined;
        state.error = undefined;
      }
    } catch (error) {
      if (error === "notPermitted") {
        state.permitted = false;
        state.actions = undefined;
        state.error = undefined;
      } else {
        state.error = typeof error === "string" ? error : "remoteFailed";
      }
    } finally {
      readingActions.delete(id);
    }
    render();
  }

  /** The devices whose actions the user can see now: the view and the device row are open. */
  function watchedActionDevices(): RemoteDevice[] {
    return watchedSessionDevices();
  }

  /** Starts or stops the reading of actions. It reads at once when it starts, and when `now` says so. */
  function watchActions(now = false): void {
    const viewing = !deviceList.closest("[hidden]") && overview !== undefined;
    const own = viewing && isOpen("this", true);
    const wanted = watchedActionDevices();
    if (!own && wanted.length === 0) {
      if (actionTicker !== undefined) window.clearInterval(actionTicker);
      actionTicker = undefined;
      return;
    }
    const read = (): void => {
      if (!deviceList.closest("[hidden]") && isOpen("this", true)) void readLocalActions();
      for (const device of watchedActionDevices()) void readActions(device.id);
    };
    const starting = actionTicker === undefined;
    actionTicker ??= window.setInterval(() => {
      if (deviceList.closest("[hidden]")) watchActions();
      else read();
    }, actionPoll);
    if (starting || now) read();
  }

  /** Reads again what a change may have touched: this device, and the devices whose actions are shown. */
  async function refreshActions(): Promise<void> {
    await Promise.all([readLocalActions(), ...watchedActionDevices().map(device => readActions(device.id))]);
  }

  /** Asks the app whether a device answers right now, and shows the answer. A check that fails leaves the last answer. */
  async function checkReach(id: string): Promise<boolean | undefined> {
    reaching.add(id);
    try {
      const online = await desktop.checkDevice(id);
      const device = overview?.devices.find(item => item.id === id);
      if (device && device.online !== online) {
        device.online = online;
        render();
      }
      return online;
    } catch {
      return undefined;
    } finally {
      reaching.delete(id);
    }
  }

  function watchingReach(): boolean {
    return !deviceList.closest("[hidden]") && (overview?.devices.length ?? 0) > 0;
  }

  /** Checks every paired device that has no check in progress, so a slow one never piles checks up. */
  function checkAll(): void {
    if (!watchingReach()) return;
    for (const device of overview!.devices) if (!reaching.has(device.id)) void checkReach(device.id);
  }

  /** Starts or stops the checks of reachability. They run while the view shows, and at once when it starts or `now` says so. */
  function watchReach(now = false): void {
    if (!watchingReach()) {
      if (reachTicker !== undefined) window.clearInterval(reachTicker);
      reachTicker = undefined;
      return;
    }
    const starting = reachTicker === undefined;
    reachTicker ??= window.setInterval(() => {
      if (watchingReach()) checkAll();
      else watchReach();
    }, reachPoll);
    if (starting || now) checkAll();
  }

  window.addEventListener("focus", () => watchReach(true));

  /** The Refresh button: checks the device now and reads its session, tools, applications, and actions again. */
  async function refreshDevice(device: RemoteDevice): Promise<void> {
    if (refreshing.has(device.id)) return;
    refreshing.add(device.id);
    render();
    const shown = new Promise(resolve => window.setTimeout(resolve, reducedMotion.matches ? 0 : refreshShown));
    const online = await checkReach(device.id);
    if (online !== false) {
      await Promise.all([
        readSession(device.id), readTools(device.id), readApps(device.id), readActions(device.id),
        // Asks the mailbox too, so a mailbox turned on there shows here at once.
        desktop.mailboxCheck({ device: device.id }).catch(() => {}),
      ]);
      void readMailboxes();
    }
    await shown;
    refreshing.delete(device.id);
    render();
  }

  /** The icons that were found, by device, application, and version. They are addresses in the asset protocol. */
  const iconSources = new Map<string, string>();
  const iconJobs: (() => Promise<void>)[] = [];
  const iconLoaders = new WeakMap<Element, () => void>();
  let iconsLoading = 0;
  // An icon is asked for only when its row shows, so a list of hundreds loads a screenful.
  const iconWatcher = new IntersectionObserver(entries => {
    for (const entry of entries) {
      const load = iconLoaders.get(entry.target);
      if (!load || !entry.isIntersecting || entry.intersectionRect.height === 0) continue;
      iconLoaders.delete(entry.target);
      iconWatcher.unobserve(entry.target);
      load();
    }
  }, { rootMargin: "60px 0px" });

  function pumpIcons(): void {
    while (iconsLoading < 2 && iconJobs.length > 0) {
      iconsLoading++;
      void iconJobs.shift()!().finally(() => {
        iconsLoading--;
        pumpIcons();
      });
    }
  }

  /** The icon of an application. The generic icon stays while the real one loads, and when it cannot. */
  function appIcon(deviceId: string, app: RemoteApp): HTMLElement {
    const root = h("span", { class: "app-icon", "data-loading": true }, icon("app-window"));
    const key = `${deviceId}/${app.bundleId}/${app.version}`;
    const show = (src: string): void => {
      const image = h("img", { src, alt: "", width: "26", height: "26", draggable: "false" });
      image.addEventListener("error", () => {
        iconSources.delete(key);
        root.replaceChildren(icon("app-window"));
        host.refreshIcons();
      });
      root.replaceChildren(image);
    };
    iconLoaders.set(root, () => {
      const known = iconSources.get(key);
      if (known) {
        root.removeAttribute("data-loading");
        show(known);
        return;
      }
      iconJobs.push(async () => {
        try {
          const src = await desktop.remoteAppIcon(deviceId, app.bundleId, app.version);
          if (src) {
            iconSources.set(key, src);
            show(src);
          }
        } catch {
          // The generic icon stays.
        } finally {
          root.removeAttribute("data-loading");
        }
      });
      pumpIcons();
    });
    iconWatcher.observe(root);
    return root;
  }

  /** The round button that opens an application. It turns into a spinner while the device opens it. */
  function appOpenButton() {
    const button = h("button", { type: "button", class: "round-button" },
      h("span", { class: "play-idle" }, icon("play")), h("span", { class: "play-busy" }, icon("refresh-cw")));
    const root = h("span", { class: "tip" }, button);
    return {
      root,
      button,
      set(label: string, tip: string, busy: boolean): void {
        setAttribute(button, "aria-label", label);
        setTip(root, tip);
        setAttribute(button, "aria-busy", String(busy));
      },
    };
  }

  function paintApp(state: HTMLElement, open: ReturnType<typeof appOpenButton>, app: RemoteApp, busy: boolean): void {
    state.toggleAttribute("data-on", app.open);
    fadeText(state, t(app.open ? "stateOpen" : "stateClosed"));
    open.set(`${t("open")} ${app.name}`, busy ? t("appsOpening") : t("open"), busy);
  }

  /** Notes the time an application was opened from here, so the tab lists the ones used last. Only the newest few are kept. */
  function rememberApp(deviceId: string, bundleId: string): void {
    const kept = Object.entries({ ...recentApps[deviceId], [bundleId]: Date.now() })
      .sort(([, a], [, b]) => b - a).slice(0, recentLimit);
    recentApps = { ...recentApps, [deviceId]: Object.fromEntries(kept) };
    persist();
  }

  /** Asks the device to open an application, shows progress on its button, and reads the list again a moment later. */
  async function openApp(deviceId: string, app: RemoteApp): Promise<void> {
    const state = appsOf(deviceId);
    if (state.opening.has(app.bundleId)) return;
    state.opening.add(app.bundleId);
    render();
    try {
      await desktop.remoteOpenApplication(deviceId, app.bundleId);
      rememberApp(deviceId, app.bundleId);
      await new Promise(resolve => window.setTimeout(resolve, openSettle));
    } catch (error) {
      if (error === "notPermitted") {
        state.permitted = false;
        state.apps = undefined;
      } else {
        host.fail(error);
      }
    } finally {
      state.opening.delete(app.bundleId);
    }
    render();
    void readApps(deviceId);
  }

  function appRow(device: { id: string }): (app: AppItem) => Row<AppItem> {
    return app => {
      const state = h("span", { class: "device-state" });
      const open = appOpenButton();
      const root = h("div", { class: "device-row" },
        appIcon(device.id, app), h("div", { class: "device-row-text" }, oneLine(h("strong", {}, app.name))), state, open.root);
      let latest = app;
      open.button.addEventListener("click", () => void openApp(device.id, latest));
      return {
        root,
        update(current) {
          latest = current;
          paintApp(state, open, current, appsOf(device.id).opening.has(current.bundleId));
        },
      };
    };
  }

  /** The rows of the Applications tab: the pinned applications, then the ones used or opened last, up to the row limit. */
  function shownApps(device: RemoteDevice, listed: readonly RemoteApp[]): AppItem[] {
    const used = recentApps[device.id] ?? {};
    const byId = new Map(listed.map(app => [app.bundleId, app]));
    const pinned = device.apps.pinned.flatMap(id => byId.has(id) ? [byId.get(id)!] : []);
    const taken = new Set(pinned.map(app => app.bundleId));
    // Opened from here comes first, newest first. The apps that the device says are open follow in its own order.
    const recent = listed.filter(app => !taken.has(app.bundleId) && used[app.bundleId] !== undefined)
      .sort((a, b) => used[b.bundleId] - used[a.bundleId]);
    for (const app of recent) taken.add(app.bundleId);
    const open = listed.filter(app => !taken.has(app.bundleId) && app.open);
    return [...pinned, ...recent, ...open].slice(0, appRows).map(app => ({ ...app, id: app.bundleId }));
  }

  function moreApps(onClick: (button: HTMLElement) => void) {
    const label = h("span", {});
    const button = h("button", { type: "button", class: "device-more", "aria-haspopup": "dialog" }, label, icon("chevron-right"));
    button.addEventListener("click", () => onClick(button));
    const root = h("div", { class: "collapse", inert: true }, h("div", {}, button));
    return {
      root,
      update(count: number): void {
        setText(label, fill(t("moreApps"), { count }));
        root.toggleAttribute("data-open", count > 0);
        root.toggleAttribute("inert", count === 0);
      },
    };
  }

  /** Saves the pins at once in the view, and on disk after. A failed save shows its notice and reads the device list again. */
  function togglePin(id: string, bundleId: string): void {
    const device = overview?.devices.find(item => item.id === id);
    if (!device) return;
    const current = device.apps.pinned;
    const next = current.includes(bundleId) ? current.filter(item => item !== bundleId) : [...current, bundleId];
    if (next.length > pinLimit) return;
    device.apps.pinned = next;
    render();
    void desktop.setPinnedApps(id, next).catch(error => {
      host.fail(error);
      void refresh();
    });
  }

  type WindowRow = { root: HTMLElement; search: string; update(app: RemoteApp, pinned: boolean, full: boolean, busy: boolean): void };

  function windowRow(device: { id: string }, app: RemoteApp): WindowRow {
    const state = h("span", { class: "device-state" });
    const open = appOpenButton();
    const pin = h("button", { type: "button", class: "icon-button outlined pin-button" }, icon("pin"));
    const pinTip = h("span", { class: "tip" }, pin);
    const root = h("li", { class: "apps-row" },
      appIcon(device.id, app), h("div", { class: "device-row-text" }, oneLine(h("strong", {}, app.name))), state, pinTip, open.root);
    let latest = app;
    open.button.addEventListener("click", () => void openApp(device.id, latest));
    pin.addEventListener("click", () => togglePin(device.id, app.bundleId));
    return {
      root,
      search: app.name.toLocaleLowerCase(),
      update(current, pinned, full, busy) {
        latest = current;
        paintApp(state, open, current, busy);
        const blocked = full && !pinned;
        const label = fill(t(pinned ? "unpinApp" : "pinApp"), { app: current.name });
        setAttribute(pin, "aria-pressed", String(pinned));
        setAttribute(pin, "aria-label", label);
        setTip(pinTip, blocked ? t("pinFull") : label);
        if (pin.disabled !== blocked) pin.disabled = blocked;
      },
    };
  }

  /** Shows the rows that match what is typed, and says why there are none when there are none. */
  function filterApps(name: string): void {
    const query = appsSearch.value.trim().toLocaleLowerCase();
    const state = appsDevice ? appsOf(appsDevice) : undefined;
    let shown = 0;
    for (const row of appsRows.values()) {
      const match = query === "" || row.search.includes(query);
      if (row.root.hidden === match) row.root.hidden = !match;
      if (match) shown++;
    }
    setText(appsCount, fill(t("appsCount"), { count: shown }));
    let message = "";
    if (state?.permitted === false) message = fill(t("appsNotAllowed"), { name });
    else if (appsRows.size === 0) message = state?.error ? t(isLinkError(state.error) ? "remoteLost" : "appsUnavailable") : t("appsLoading");
    else if (shown === 0) message = t("appsEmpty");
    if (message) fadeText(appsNote, message);
    setHidden(appsNote, message === "");
  }

  /** The window of every application of a device. Its rows are kept and updated in place, and rebuilt only when the list or the language changes. */
  function renderAppsWindow(): void {
    if (!appsWindow.hasAttribute("data-open") || !appsDevice || !overview) return;
    const device = overview.devices.find(item => item.id === appsDevice);
    if (!device) {
      closeApps(false);
      return;
    }
    const state = appsOf(device.id);
    const apps = state.apps ?? [];
    const signature = JSON.stringify([host.language(), device.id, apps.map(app => [app.bundleId, app.name, app.version])]);
    if (signature !== appsSignature) {
      appsSignature = signature;
      appsRows = new Map(apps.map(app => [app.bundleId, windowRow(device, app)]));
      appsList.replaceChildren(...[...appsRows.values()].map(row => row.root), appsNote);
      host.refreshIcons();
    }
    const pinned = new Set(device.apps.pinned);
    for (const app of apps) {
      appsRows.get(app.bundleId)?.update(app, pinned.has(app.bundleId), pinned.size >= pinLimit, state.opening.has(app.bundleId));
    }
    filterApps(device.name);
  }

  function openApps(device: RemoteDevice, trigger: HTMLElement): void {
    appsDevice = device.id;
    appsTrigger = trigger;
    appsSearch.value = "";
    appsSignature = "";
    appsWindow.setAttribute("data-open", "");
    appsWindow.inert = false;
    renderAppsWindow();
    watchApps(true);
    appsSearch.focus();
  }

  function closeApps(restoreFocus = true): void {
    if (!appsWindow.hasAttribute("data-open")) return;
    appsWindow.removeAttribute("data-open");
    appsWindow.inert = true;
    appsDevice = undefined;
    watchApps();
    if (restoreFocus && appsTrigger?.isConnected) appsTrigger.focus();
  }

  appsClose.addEventListener("click", () => closeApps());
  appsWindow.addEventListener("pointerdown", event => {
    if (event.target === appsWindow) closeApps();
  });
  appsSearch.addEventListener("input", () => {
    const device = overview?.devices.find(item => item.id === appsDevice);
    if (device) filterApps(device.name);
  });
  bind(appsSearch, "appsSearch", "placeholder");
  bind(appsSearch, "appsSearch", "aria-label");

  function remoteBody(device: RemoteDevice) {
    const key = `r:${device.id}`;
    const toolRows = h("div", {});
    const toolList = list<RemoteTool>(toolRows, tool => JSON.stringify([host.language(), tool.id, device.name]),
      tool => toolRow(device, tool));
    const appHost = h("div", {});
    const appList = list<AppItem>(appHost, app => JSON.stringify([host.language(), app.name, app.version]), appRow(device));
    let latest = device;
    const more = moreApps(button => openApps(latest, button));
    const actionHost = h("div", {});
    const actionList = list<SavedAction>(actionHost, actionSignature, actionRow(device));
    const actionSearch = searchField("searchActions", () => render());
    const toolsBody = tabBody(toolRows);
    const appsBody = tabBody(appHost, more.root);
    const actionsBody = tabBody(actionHost);

    const shellNote = h("span", { class: "setting-note" });
    const terminal = iconButton(t("openTerminalDefault"), "terminal", "outlined");
    const copy = iconButton(t("copyShellCommand"), "copy", "outlined");
    const copied = h("span", { class: "copy-confirm", role: "status" });
    let copiedTimer: number | undefined;
    terminal.button.addEventListener("click", () => {
      void desktop.openShell(latest.id).catch(error => host.fail(error));
    });
    copy.button.addEventListener("click", () => {
      // The app writes the pasteboard. The webview loses the click after an awaited call, so it could not.
      void desktop.copyShellCommand(latest.id).then(() => {
        setText(copied, t("shellCommandCopied"));
        copied.setAttribute("data-show", "");
        window.clearTimeout(copiedTimer);
        copiedTimer = window.setTimeout(() => copied.removeAttribute("data-show"), copiedFor);
      }).catch(error => host.fail(error));
    });
    // Each way into the other device is one row. A new way is another row in this list.
    const terminalRow = h("div", { class: "device-row" },
      h("span", { class: "action-icon" }, icon("square-terminal")),
      h("div", { class: "device-row-text" }, h("strong", {}, t("connectTerminal")), shellNote),
      h("div", { class: "device-end shell-buttons" }, copied, terminal.root, copy.root));
    const screenState = h("span", { class: "setting-note" });
    const screen = iconButton(t("openScreenTip"), "monitor-up", "outlined");
    screen.button.addEventListener("click", () => {
      void desktop.openScreen(latest.id).catch(error => host.fail(error));
    });
    const screenRow = h("div", { class: "device-row" },
      h("span", { class: "action-icon" }, icon("monitor")),
      h("div", { class: "device-row-text" }, h("strong", {}, t("connectScreen")), screenState),
      h("div", { class: "device-end shell-buttons" }, screen.root));
    const filesState = h("span", { class: "setting-note" });
    const filesOpen = iconButton(t("openFilesTip"), "folder-open", "outlined");
    filesOpen.button.addEventListener("click", () => explorer.open({ id: latest.id, name: latest.name }, filesOpen.button));
    const filesRow = h("div", { class: "device-row" },
      h("span", { class: "action-icon" }, icon("folder")),
      h("div", { class: "device-row-text" }, h("strong", {}, t("connectFiles")), filesState),
      h("div", { class: "device-end shell-buttons" }, filesOpen.root));
    const connectBody = tabBody(h("div", { class: "device-rows" }, terminalRow, screenRow, filesRow));

    const tabs = tabset(key, fill(t("tabListOther"), { name: device.name }), [
      { id: "tools", title: "tools", panel: tabColumn(toolsBody) },
      { id: "apps", title: "sectionApplications", panel: tabColumn(appsBody) },
      { id: "actions", title: "sectionActions", panel: tabColumn(actionsBody, h("div", { class: "tab-head" }, actionSearch.root)) },
      { id: "connect", title: "tabConnect", panel: tabColumn(connectBody) },
    ], true, () => {});

    const offline = h("p", { class: "setting-note device-offline" }, t("deviceOfflineNote"));
    const root = h("div", { class: "device-body", "data-remote": "" }, tabs.root, offline);

    return {
      root,
      update(current: RemoteDevice): void {
        only([tabs.root, offline], [current.online === true ? tabs.root : offline]);
        const found = live.get(current.id);
        const known = (found?.tools ?? current.tools).filter(tool => tools.some(({ id }) => id === tool.id));
        toolList.sync(known);
        // A device that refuses, or a link that fails, says so where the tools would count.
        const toolsError = found?.error
          ? (isLinkError(found.error) ? t("remoteLost") : fill(t(found.error as "localOnly"), { name: current.name }))
          : "";
        toolsBody.say(toolsError || (known.length === 0 ? t("toolsNone") : ""), toolsError !== "");
        tabs.summary("tools", toolsError ? "" : fill(t("tabToolsSummary"), { on: known.filter(tool => tool.running).length, total: known.length }));

        latest = current;
        const state = appsOf(current.id);
        const listed = state.apps ?? [];
        const denied = state.permitted === false;
        const shown = denied ? [] : shownApps(current, listed);
        appList.sync(shown);
        more.update(denied ? 0 : Math.max(0, listed.length - shown.length));
        // The device says whether it allowed applications. Until it answers, the tab says that it is reading.
        let message = "";
        if (denied) message = fill(t("appsNotAllowed"), { name: current.name });
        else if (!state.apps) message = state.error ? t(isLinkError(state.error) ? "remoteLost" : "appsUnavailable") : t("appsLoading");
        if (!message && listed.length === 0) message = t("appsNone");
        appsBody.say(message);
        tabs.summary("apps", denied ? t("off") : state.apps ? String(listed.filter(app => app.open).length) : "");

        const actionState = actionsOf(current.id);
        const actionsDenied = actionState.permitted === false;
        const listedActions = actionsDenied ? [] : actionState.actions ?? [];
        const matching = filterActions(listedActions, actionSearch.query());
        actionList.sync(matching);
        // The device says whether it allowed actions. Until it answers, the tab says that it is reading.
        let actionsMessage = "";
        if (actionsDenied) actionsMessage = fill(t("actionsNotAllowed"), { name: current.name });
        else if (!actionState.actions) {
          actionsMessage = actionState.error ? t(isLinkError(actionState.error) ? "remoteLost" : "actionsUnavailable") : t("actionsLoading");
        }
        if (!actionsMessage && actionState.actions) {
          actionsMessage = listedActions.length === 0 ? t("remoteActionsNone") : matching.length === 0 ? t("actionsNoMatch") : "";
        }
        actionsBody.say(actionsMessage);
        tabs.summary("actions", actionsDenied ? t("off") : !actionState.actions ? "" : String(listedActions.length));

        // The device says whether its shell is on. Only the person at that device turns it on.
        const grant = found?.shell ?? null;
        const on = grant !== null && (grant.until === "untilOff" || grant.until > seconds());
        const until = !on ? t("off") : grant.until === "untilOff" ? t("deviceShellOnUntilOff")
          : fill(t("deviceShellOn"), { time: formatClock(grant.until, host.language()) });
        // The device says what it shares. Only the person at that device turns it on.
        const sharing = found?.files?.on === true && (found.files.until === "untilOff" || found.files.until > seconds()) ? found.files : undefined;
        // The tab shows a dot while a way in is on, and nothing while all are off.
        tabs.summary("connect", on || sharing ? t("tabOn") : "", on || sharing !== undefined);
        fadeText(shellNote, on ? until : t("connectOff"));
        terminal.disable(on ? undefined : t("connectOff"), t("openTerminalTip"));
        copy.disable(on ? undefined : t("connectOff"), t("copyShellCommand"));

        // The device says whether Screen Sharing is on. Only the person at that device turns it on.
        const screenFound = found?.screen;
        const screenError = found?.screenError;
        const screenText = screenError
          ? (isLinkError(screenError) ? t("remoteLost") : screenError === "notPermitted" ? t("screenNotAllowed") : isRefusal(screenError) ? fill(t(screenError), { name: current.name }) : t("screenOpenFailed"))
          : !screenFound ? t("screenReading")
          : !screenFound.on ? t("screenOffThere")
          : screenFound.userAllowed === false ? t("screenNoUserThere") : t("screenOn");
        const screenReady = !screenError && screenFound?.on === true && screenFound.userAllowed !== false;
        const filesError = found?.filesError;
        const filesText = filesError
          ? (isLinkError(filesError) ? t("remoteLost") : isRefusal(filesError) ? fill(t(filesError), { name: current.name }) : t("filesIo"))
          : !found?.files ? t("filesReading") : !sharing ? t("filesOffThere") : filesSummary(sharing);
        const filesReady = !filesError && sharing !== undefined;
        fadeText(filesState, filesText);
        filesOpen.disable(filesReady ? undefined : filesText, t("openFilesTip"));
        setAttribute(filesOpen.button, "aria-label", filesReady ? t("openFilesTip") : filesText);
        fadeText(screenState, screenText);
        screen.disable(screenReady ? undefined : screenText, t("openScreenTip"));
        setAttribute(screen.button, "aria-label", screenReady ? t("openScreenTip") : screenText);
      },
    };
  }

  /** `2 shared folders · read and write`, `Home folder · read only`, or `Whole disk · read only`. */
  function filesSummary(files: FileShare & { on: true }): string {
    const mode = t(files.mode === "read" ? "filesModeReadShort" : "filesModeWriteShort");
    if (files.scope === "disk") return fill(t("filesOnDisk"), { mode });
    if (files.scope === "home") return fill(t("filesOnHome"), { mode });
    return files.roots.length === 1 ? fill(t("filesOnFolder"), { mode }) : fill(t("filesOnFolders"), { mode, count: files.roots.length });
  }

  function deviceRow(item: Item): Row<Item> {
    const name = h("strong", {}, item.name || t("thisDevice"));
    const state = h("span", { class: "device-state" });
    const open = iconButton(t("openInterface"), "square-arrow-out-up-right", "outlined");
    const refreshButton = iconButton(t("refreshDevice"), "refresh-cw", "outlined");
    const forget = iconButton(t("forgetDevice"), "user-minus");
    const mailbox = mailboxButton();
    const remote = item.self ? undefined : item.data as RemoteDevice;
    let latestRemote = remote;
    mailbox.button.addEventListener("click", () => {
      if (latestRemote) host.openMailbox({ device: latestRemote.id }, latestRemote.name, latestRemote.online === true, mailbox.button);
    });
    const { accept, cancel } = confirmButtons(t("forgetAccept"), () => { if (remote) void forgetRemote(remote); }, () => {
      confirming = undefined;
      render();
      forget.button.focus();
    });
    const confirm = h("div", { class: "device-confirm", hidden: true },
      h("span", { class: "setting-note" }, t("forgetConfirm")), accept.root, cancel.root);
    open.button.addEventListener("click", () => {
      if (remote) void desktop.openRemoteWindow(remote.id).catch(error => host.fail(error));
    });
    refreshButton.button.addEventListener("click", () => {
      if (remote) void refreshDevice(remote);
    });
    forget.button.addEventListener("click", () => {
      if (!remote) return;
      confirming = `forget:${remote.id}`;
      render();
      cancel.button.focus();
    });
    const nameTip = oneLine(name);
    // The tip lies above the row's toggle, so a click on the name goes on to it.
    nameTip.addEventListener("click", () => head.querySelector<HTMLElement>(".device-toggle")?.click());
    const head = h("div", { class: "device-head" },
      kindIcon(item.kind),
      h("span", { class: "device-name" }, nameTip, item.self && h("span", { class: "device-badge" }, t("thisDevice"))),
      !item.self && state, !item.self && refreshButton.root, !item.self && open.root, !item.self && mailbox.root, !item.self && forget.root, !item.self && confirm, chevron());
    const content = item.self ? thisDeviceBody() : remoteBody(item.data as RemoteDevice);
    const body = h("div", { class: "collapse" }, content.root);
    disclosure(item.id, head, name, body, item.self);
    const root = h("div", { class: "device" }, head, body);
    return {
      root,
      update(current) {
        if (current.self) {
          (content as ReturnType<typeof thisDeviceBody>).update(current.data as DevicesOverview["thisDevice"]);
          return;
        }
        const device = current.data as RemoteDevice;
        latestRemote = device;
        const asking = confirming === `forget:${device.id}`;
        // The mailbox is always there. Until that device allows it for this one, it is off and says where to turn it on.
        const box = mailboxOf({ device: device.id });
        const normal = [state, refreshButton.root, open.root, mailbox.root, forget.root];
        only([...normal, confirm], asking ? [confirm] : normal);
        // The tip of an off button explains it in the words of the mailbox window: off there, not reached, or not answered yet.
        const blocked = mailboxBlocked(box?.granted ?? null, box?.check ?? null, device.name, t, host.language());
        // A device that allows it but was not reached keeps the button on, so what came before stays readable.
        mailbox.set(box?.unread ?? 0, box?.granted === true ? undefined
          : blocked ? `${blocked.text} ${blocked.checked}`.trim() : fill(t("mailboxNotOn"), { name: device.name }));
        const checking = refreshing.has(device.id);
        state.toggleAttribute("data-on", device.online === true && !checking);
        fadeText(state, checking ? t("deviceChecking") : t(device.online === true ? "deviceOnline" : "deviceOffline"));
        setAttribute(refreshButton.button, "aria-busy", String(checking));
        open.disable(device.online === true ? undefined : t("deviceOfflineTip"), t("openInterface"));
        (content as ReturnType<typeof remoteBody>).update(device);
      },
    };
  }

  const devices = list<Item>(deviceList, item => JSON.stringify([host.language(), item.self, item.name, item.kind]), deviceRow);

  function render(): void {
    if (!overview) return;
    const { thisDevice } = overview;
    addTip.removeAttribute("data-tip");
    if (confirming?.startsWith("forget:") && !overview.devices.some(({ id }) => `forget:${id}` === confirming)) confirming = undefined;
    devices.sync([
      { id: "this", self: true, name: thisDevice.name, kind: thisDevice.kind, data: thisDevice },
      ...overview.devices.map(device => ({
        id: `r:${device.id}`, self: false, name: device.name, kind: device.kind, data: device,
      })),
    ]);
    renderAppsWindow();
    if (shownLanguage !== host.language()) {
      shownLanguage = host.language();
      retranslate();
    } else {
      renderPairWindow();
    }
    syncTicker();
    watchTools();
    watchApps();
    watchActions();
    watchSessions();
    watchLog();
    watchReach();
    if (built) {
      built = false;
      host.refreshIcons();
    }
  }

  async function refresh(): Promise<void> {
    const current = ++requested;
    try {
      const next = await desktop.getDevices();
      if (current !== requested) return;
      overview = next;
    } catch {
      // Keep the last list. A first failure still shows this device, without its details.
      overview ??= {
        thisDevice: {
          name: "", kind: "desktop",
          remoteAccess: { remoteLogin: "off", passwordLogin: "unknown", userAllowed: null, managedFile: false, macosMajor: null },
          pairing: { open: false, expires: null, attemptsLeft: 0 }, account: null, home: null, controllers: [],
          socket: { listening: true, error: null }, warnings: [],
        },
        devices: [],
      };
    }
    render();
    void readLocalActions();
    void readLocalScreen();
    void readMailboxes();
  }

  /** Reads the mailbox summary and shows its counts. */
  async function readMailboxes(): Promise<void> {
    try {
      mailboxes = await desktop.mailboxSummary();
      render();
    } catch {
      // The buttons keep what they showed.
    }
  }

  addButton.setAttribute("aria-haspopup", "dialog");
  addButton.addEventListener("click", () => openAdd(addButton));
  addButton.disabled = false;
  host.refreshIcons();

  return { refresh, render, reset, readMailboxes };
}
