import "./devices.css";
import {
  desktop, type ActionSchedule, type Controller, type ControllerPermission, type DeviceKind, type DevicesOverview,
  type DiscoveredDevice, type ProxyNetwork, type RemoteApp, type RemoteDevice, type RemoteLogEntry, type RemoteTool, type SavedAction,
} from "./desktop";
import guideImage13 from "./assets/guide/remote-login-13.png";
import { messages, type Language, type Message } from "./messages";
import { tools } from "./modules";
import { h, icon, type Child } from "./proxy";
import { age, formatClock, formatMoment, formatSession, type AgeUnit } from "./time";

// Every name and text below comes from data that other devices will send, so it is only ever set as text.
const openKey = "lazy-worker-devices-open";
const appRows = 3;
const closeDelay = 300;
const settleDelay = 260;
const ageMessages: Record<Exclude<AgeUnit, "seconds">, Message> = {
  minutes: "minutesAgo", hours: "hoursAgo", days: "daysAgo",
};
const permissions: { id: ControllerPermission; icon: string; label: Message }[] = [
  { id: "control", icon: "layout-grid", label: "permControl" },
  { id: "applications", icon: "app-window", label: "permApplications" },
  { id: "actions", icon: "play", label: "permActions" },
  { id: "shell", icon: "terminal", label: "permShell" },
];
/** The request names and outcomes that the log shows in words. Any other one shows as it is. */
const logEvents: Record<string, Message> = { removePeer: "remoteLogEventRemovePeer", cutAll: "remoteLogEventCutAll" };
const logOutcomes: Record<string, Message> = {
  ok: "remoteOutcomeOk", notPermitted: "remoteOutcomeNotPermitted", localOnly: "remoteOutcomeLocalOnly",
  notPaired: "remoteOutcomeNotPaired", badRequest: "remoteOutcomeBadRequest",
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
};
type Row<T> = { root: HTMLElement; update(item: T): void };
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

/** The open rows and sections are a per-viewer convenience: the view works without storage. */
function readOpen(): Record<string, boolean> {
  try {
    const stored: unknown = JSON.parse(localStorage.getItem(openKey) ?? "{}");
    if (!stored || typeof stored !== "object") return {};
    return Object.fromEntries(Object.entries(stored).filter(([, open]) => typeof open === "boolean"));
  } catch {
    return {};
  }
}

export function mountDevices(host: Host) {
  const t = host.translate;
  const deviceList = document.getElementById("device-list")!;
  const addButton = document.getElementById("add-device") as HTMLButtonElement;
  const addTip = addButton.parentElement!;

  const logWindow = document.getElementById("device-log-window")!;
  const logList = document.getElementById("device-log-list")!;
  const logCount = document.getElementById("device-log-count")!;
  const logClose = document.getElementById("device-log-close") as HTMLButtonElement;

  const guideWindow = document.getElementById("remote-guide-window")!;
  const guideBody = document.getElementById("remote-guide-body")!;
  const guideClose = document.getElementById("remote-guide-close") as HTMLButtonElement;
  const guideOpen = document.getElementById("remote-guide-open") as HTMLButtonElement;

  let overview: DevicesOverview | undefined;
  let enabling = false;
  let guideTrigger: HTMLElement | undefined;
  let requested = 0;
  /** What waits for a second click: a device id, or `cut` for all access. Only one thing waits at a time. */
  let confirming: string | undefined;
  let logEntries: RemoteLogEntry[] = [];
  let logTrigger: HTMLElement | undefined;
  let opened = readOpen();
  let built = false;
  let sequence = 0;
  let shownLanguage: Language | undefined;

  function isOpen(key: string, fallback = false): boolean {
    return opened[key] ?? fallback;
  }

  function remember(key: string, open: boolean): void {
    opened = { ...opened, [key]: open };
    try {
      localStorage.setItem(openKey, JSON.stringify(opened));
    } catch {
      // Without storage the rows still open and close. They only start closed next time.
    }
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
        setText(paragraph, value);
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
    apply(isOpen(key, fallback), false);
    toggle.addEventListener("click", () => {
      const open = !body.hasAttribute("data-open");
      remember(key, open);
      apply(open, true);
    });
    head.prepend(toggle);
    return toggle;
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

  function weekday(day: number): string {
    const index = Number.isFinite(day) ? ((Math.trunc(day) % 7) + 7) % 7 : 0;
    // 7 January 2024 is a Sunday.
    return new Date(2024, 0, 7 + index).toLocaleDateString(host.language(), { weekday: "long" });
  }

  function scheduleText(schedule: ActionSchedule): string {
    if (schedule.every === "daily") return fill(t("actionDaily"), { time: schedule.time });
    if (schedule.every === "weekly") return fill(t("actionWeekly"), { day: weekday(schedule.weekday), time: schedule.time });
    return t("actionManual");
  }

  function actionText(action: SavedAction): string {
    const last = action.lastRun
      ? fill(t(action.lastRun.ok ? "actionLastOk" : "actionLastFailed"), { age: elapsed(action.lastRun.time) }) : "";
    return [scheduleText(action.schedule), last].filter(Boolean).join(" · ");
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

  function actionRow(chevronAfter: boolean) {
    return (action: SavedAction): Row<SavedAction> => {
      const name = h("strong", {}, action.name);
      const summary = h("span", { class: "setting-note" });
      const schedule = iconButton(fill(t("scheduleAction"), { action: action.name }), "calendar-clock", "outlined");
      const run = playButton(fill(t("runAction"), { action: action.name }), t("runAction").replace(" {action}", ""));
      const root = h("div", { class: "device-row" },
        h("span", { class: "action-icon" }, icon("square-terminal")),
        h("div", { class: "device-row-text" }, name, summary),
        schedule.root, run.root,
        chevronAfter && icon("chevron-right"));
      return { root, update: current => setText(summary, actionText(current)) };
    };
  }

  const actionSignature = (action: SavedAction): string => JSON.stringify([host.language(), action.name]);

  /** Saves the three permissions that can change here. The shell is turned on at the device itself. */
  async function setPermission(controller: Controller, id: ControllerPermission, on: boolean): Promise<void> {
    const next = { ...controller.permissions, [id]: on };
    try {
      await desktop.setPeerPermissions(controller.id, {
        control: next.control, applications: next.applications, actions: next.actions,
      });
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

  function controllerRow(controller: Controller): Row<Controller> {
    let current = controller;
    const detail = h("span", { class: "setting-note" });
    const chips = permissions.map(permission => {
      const locked = permission.id === "shell";
      const chip = h("button", {
        type: "button", class: "permission-chip", "aria-pressed": "false", disabled: locked,
        "aria-label": locked ? `${t(permission.label)}. ${t("permShellLocked")}` : t(permission.label),
      }, icon(permission.icon), h("span", {}, t(permission.label)));
      if (!locked) {
        chip.addEventListener("click", () => {
          void setPermission(current, permission.id, chip.getAttribute("aria-pressed") !== "true");
        });
      }
      // The wrapper holds the tip, so the disabled shell chip can still say why it is off.
      const root = locked ? h("span", { class: "tip", "data-tip": t("permShellLocked") }, chip) : chip;
      return { permission, chip, root };
    });
    const chipBox = h("div", { class: "permission-chips" }, ...chips.map(({ root }) => root));
    const remove = iconButton(fill(t("removeController"), { device: controller.name }), "user-minus");
    const { accept, cancel } = confirmButtons(t("removeAccess"), () => { void removeController(current); }, () => {
      confirming = undefined;
      render();
      remove.button.focus();
    });
    const confirm = h("div", { class: "device-confirm", hidden: true },
      h("span", { class: "setting-note" }, t("removeConfirm")), accept.root, cancel.root);
    remove.button.addEventListener("click", () => {
      confirming = current.id;
      render();
      cancel.button.focus();
    });
    const root = h("div", { class: "device-row" },
      kindIcon(controller.kind),
      h("div", { class: "device-row-text" }, h("strong", {}, controller.name), detail),
      chipBox, remove.root, confirm);
    return {
      root,
      update(next) {
        current = next;
        setText(detail, next.connected ? t("connectedNow")
          : next.lastAccess === null ? t("noAccessYet") : fill(t("lastAccess"), { age: elapsed(next.lastAccess) }));
        for (const { permission, chip } of chips) {
          setAttribute(chip, "aria-pressed", String(next.permissions[permission.id] === true));
          chip.toggleAttribute("data-on", next.permissions[permission.id] === true);
        }
        const asking = confirming === next.id;
        chipBox.hidden = asking;
        remove.root.hidden = asking;
        confirm.hidden = !asking;
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
    const enableLabel = h("span", {}, t("remoteAccessEnable"));
    const enable = h("button", { type: "button", class: "access-action", "data-attention": "" }, icon("power"), enableLabel);
    const sharing = iconButton(t("openSharingSettings"), "square-arrow-out-up-right", "outlined");
    const passwords = iconButton(t("allowPasswordsAgain"), "lock-keyhole-open");
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
        h("div", { class: "access-text" }, accessLabel, accessNote),
        h("div", { class: "device-end" }, enable, sharing.root, passwords.root, passwordConfirm)),
    };
    enable.addEventListener("click", () => { void enableAccess(enable); });
    sharing.button.addEventListener("click", () => { void openSharing(); });
    passwords.button.addEventListener("click", () => {
      confirming = "passwords";
      render();
      passwordButtons.cancel.button.focus();
    });
    const pairingState = h("span", { class: "setting-note pairing-state" });
    const pairing = iconButton(t("openPairing"), "lock-keyhole-open", "outlined");
    const pairingRow = settingRow("lock-keyhole", "pairing", pairingState, pairing.root);
    pairing.button.setAttribute("aria-haspopup", "dialog");
    pairing.button.addEventListener("click", () => openPairWindow(pairing.button));

    const socketNote = note("");
    const controllersEmpty = note(t("noControllers"));
    const controllerRows = h("div", { class: "device-rows" });
    const controllerList = list<Controller>(
      controllerRows, controller => JSON.stringify([host.language(), controller.name, controller.kind]), controllerRow);

    const addAction = iconButton(t("addAction"), "plus", "outlined");
    const actionsEmpty = note(t("noActions"));
    const actionRows = h("div", { class: "device-rows" });
    const actionList = list<SavedAction>(actionRows, actionSignature, actionRow(true));

    const log = iconButton(t("showLog"), "maximize-2");
    const logRow = settingRow("list", "deviceLog", log.root);

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
    log.button.setAttribute("aria-haspopup", "dialog");
    log.button.addEventListener("click", () => { void openLog(log.button); });

    const root = h("div", { class: "device-body" },
      access.root, pairingRow.root,
      h("div", { class: "device-subhead" }, h("h3", {}, t("whoCanControl"))),
      controllerRows, controllersEmpty.root, socketNote.root,
      h("div", { class: "device-subhead" }, h("h3", {}, t("deviceActions")), addAction.root),
      actionRows, actionsEmpty.root,
      logRow.root, cut);

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
        setText(accessMessage, t(note));
        warnIcon.hidden = !(noUser || remote.passwordLogin === "allowed");
        okIcon.hidden = needsTurnOn;
        accessNote.toggleAttribute("data-warning", !warnIcon.hidden);
        setTip(accessInfo, t("remoteAccessHelp"));
        setAttribute(accessInfo, "aria-label", t("remoteAccessHelp"));
        setText(enableLabel, t(enabling ? "remoteAccessEnabling" : "remoteAccessEnable"));
        enable.hidden = !needsTurnOn;
        if (enable.disabled !== enabling) enable.disabled = enabling;
        enable.toggleAttribute("data-attention", !enabling);
        enable.setAttribute("aria-busy", String(enabling));
        sharing.root.hidden = !(off || noUser);
        const canAllow = !needsTurnOn && remote.managedFile;
        if (confirming === "passwords" && !canAllow) confirming = undefined;
        passwords.root.hidden = !canAllow || confirming === "passwords";
        passwordConfirm.hidden = !(canAllow && confirming === "passwords");
        if (guideWindow.hasAttribute("data-open")) {
          if (off || noUser) renderGuide();
          else closeGuide(false);
        }
        setText(pairingState, data.pairing.open && data.pairing.expires !== null
          ? fill(t("pairingOpenLeft"), { time: formatSession(data.pairing.expires - seconds()) }) : t("pairingClosed"));
        const pairingName = t(data.pairing.open ? "showPairing" : "openPairing");
        pairing.disable(needsTurnOn ? t("pairNeedsAccess") : undefined, pairingName);
        setAttribute(pairing.button, "aria-label", needsTurnOn ? t("pairNeedsAccess") : pairingName);
        // Saved actions arrive in a later step, so the app shows them off until then.
        const later = desktop.sampleDevices ? undefined : t("notAvailableYet");
        addAction.disable(later, t("addAction"));
        log.disable(undefined, t("showLog"));
        if (confirming && confirming !== "cut" && confirming !== "passwords" && !confirming.startsWith("forget:") &&
          !data.controllers.some(({ id }) => id === confirming)) confirming = undefined;
        if (data.controllers.length === 0 && confirming === "cut") confirming = undefined;
        controllerList.sync(data.controllers);
        controllersEmpty.show(data.controllers.length === 0);
        const reason = data.socket.error;
        socketNote.set(fill(t("socketUnavailable"), {
          reason: reason && Object.prototype.hasOwnProperty.call(messages.en, reason) ? t(reason as Message) : reason ?? "",
        }));
        socketNote.show(!data.socket.listening);
        actionList.sync(data.actions);
        actionsEmpty.show(data.actions.length === 0);
        const nobody = data.controllers.length === 0;
        const asking = confirming === "cut";
        if (cutButton.disabled !== nobody) cutButton.disabled = nobody;
        if (nobody) setTip(cutTip, t("noControllers"));
        else cutTip.removeAttribute("data-tip");
        cutTip.hidden = asking;
        cutAsk.root.hidden = !asking;
      },
    };
  }

  async function cutAll(): Promise<void> {
    confirming = undefined;
    try {
      await desktop.cutAllAccess();
    } catch (error) {
      host.fail(error);
    }
    await refresh();
  }

  function outcomeText(outcome: string): string {
    const known = logOutcomes[outcome];
    if (known) return t(known);
    return Object.prototype.hasOwnProperty.call(messages.en, outcome) ? t(outcome as Message) : outcome;
  }

  /** Names and request names come from other devices, so they are only ever set as text. */
  function logItem(entry: RemoteLogEntry): HTMLElement {
    const event = logEvents[entry.request];
    return h("li", { class: "proxy-log-entry", "data-outcome": entry.outcome === "ok" ? "ok" : "refused" },
      h("time", {}, formatMoment(entry.time, host.language())),
      h("span", { class: "proxy-log-target" }, entry.name ?? "—"),
      h("span", { class: "proxy-log-target" }, event ? t(event) : entry.request),
      h("span", { class: "proxy-log-outcome" }, outcomeText(entry.outcome)));
  }

  function renderLog(): void {
    setText(logCount, fill(t("remoteLogEntries"), { count: logEntries.length }));
    logList.replaceChildren(...(logEntries.length
      ? logEntries.map(logItem) : [h("li", { class: "setting-note" }, t("remoteLogEmpty"))]));
  }

  async function openLog(trigger: HTMLElement): Promise<void> {
    logTrigger = trigger;
    try {
      logEntries = await desktop.getRemoteLog();
    } catch (error) {
      host.fail(error);
      return;
    }
    renderLog();
    logWindow.setAttribute("data-open", "");
    logWindow.inert = false;
    trigger.setAttribute("aria-expanded", "true");
    logClose.focus();
  }

  function closeLog(restoreFocus = true): void {
    if (!logWindow.hasAttribute("data-open")) return;
    logWindow.removeAttribute("data-open");
    logWindow.inert = true;
    logTrigger?.setAttribute("aria-expanded", "false");
    if (restoreFocus && logTrigger?.isConnected) logTrigger.focus();
  }

  logClose.addEventListener("click", () => closeLog());
  logWindow.addEventListener("pointerdown", event => {
    if (event.target === logWindow) closeLog();
  });
  document.addEventListener("keydown", event => {
    if (event.key !== "Escape") return;
    if (addOverlay.isOpen()) addOverlay.close();
    else if (pairOverlay.isOpen()) pairOverlay.close();
    else if (guideWindow.hasAttribute("data-open")) closeGuide();
    else if (logWindow.hasAttribute("data-open")) closeLog();
    else if (confirming) {
      confirming = undefined;
      render();
    }
  });

  /** The steps for the user to do in Sharing, when Lazy Worker could not turn remote access on by itself. */
  function renderGuide(): void {
    const access = overview?.thisDevice.remoteAccess;
    if (!access) return;
    const off = access.remoteLogin === "off";
    const picture = off && access.macosMajor !== null && access.macosMajor >= 13;
    const steps: Message[] = ["remoteGuideOpen"];
    if (off) steps.push(access.macosMajor !== null && access.macosMajor <= 12 ? "remoteGuideTick" : "remoteGuideSwitch");
    steps.push("remoteGuideUser");
    guideBody.replaceChildren(h("div", {},
      h("ol", { class: "guide-steps" }, ...steps.map(step => h("li", {}, t(step)))),
      picture && h("img", {
        class: "guide-image", src: guideImage13, alt: t("remoteGuideImageAlt"), width: "470", height: "406",
      })));
  }

  function openGuide(trigger: HTMLElement | undefined): void {
    guideTrigger = trigger;
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
  guideOpen.addEventListener("click", () => { void openSharing(); });

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
  let selected: { name: string; host: string; port: number } | undefined;
  let pairBusy = false;
  let pairLocked = false;
  let pairErrorKey: Message | undefined;
  /** Changes when the window closes or goes back, so the answer of an old pairing does not touch the new screen. */
  let addAttempt = 0;

  const rescan = iconButton(t("addSearchAgain"), "refresh-cw", "outlined");
  bindTip(rescan, "addSearchAgain");
  const foundNote = note("");
  const foundRows = h("div", { class: "device-rows" });
  const foundList = list<Found>(foundRows, device => JSON.stringify([host.language(), device.name, device.host, device.port]), foundRow);
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
    foundNote.root, foundRows,
    h("div", { class: "device-subhead" }, bind(h("h3"), "addEnterAddress")), addressForm);

  const summaryName = h("strong");
  const summaryWhere = h("span", { class: "setting-note" });
  const accountField = h("input", {
    class: "field pair-field", type: "text", autocomplete: "username", autocapitalize: "off", spellcheck: "false", maxlength: "32",
  });
  const accountInfo = h("span", { class: "tip option-info", tabindex: "0", role: "img" }, icon("info"));
  bind(accountInfo, "addAccountHelp", "data-tip");
  bind(accountInfo, "addAccountHelp", "aria-label");
  const account = overlayRow("user-round", "addAccount", "label", accountField);
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
    const where = device.port === 22 ? device.host : `${device.host}:${device.port}`;
    const root = h("button", { type: "button", class: "device-row found-row" },
      kindIcon("desktop"),
      h("div", { class: "device-row-text" }, h("strong", {}, device.name), h("span", { class: "setting-note" }, where)),
      icon("chevron-right"));
    root.addEventListener("click", () => choose({ name: device.name, host: device.host, port: device.port }));
    return { root, update: () => undefined };
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
    foundList.sync(found);
    const text = scanning && !found.length ? t("addSearching")
      : scanError ? t(scanError) : found.length ? "" : t("addNoneFound");
    if (text) foundNote.set(text);
    foundNote.show(text !== "");
    settleIcons();
  }

  async function scan(): Promise<void> {
    const current = ++scanSequence;
    scanning = true;
    renderFound();
    try {
      const result = await desktop.discoverDevices();
      if (current !== scanSequence) return;
      found = result.map(device => ({ ...device, id: `${device.host}:${device.port}` }));
      scanError = undefined;
    } catch {
      if (current !== scanSequence) return;
      found = [];
      scanError = "discoveryFailed";
    }
    scanning = false;
    renderFound();
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
    setText(pairLabel, t(pairBusy ? "addPairing" : "addPair"));
    const ready = addAccountValid(user) && digitsOf(codeField.value).length === 6 && !pairBusy && !pairLocked;
    if (pairButton.disabled !== !ready) pairButton.disabled = !ready;
    pairButton.setAttribute("aria-busy", String(pairBusy));
    for (const field of [accountField, codeField]) field.readOnly = pairBusy;
    setText(summaryName, selected?.name ?? "");
    setText(summaryWhere, selected ? (selected.port === 22 ? selected.host : `${selected.host}:${selected.port}`) : "");
  }

  function showStep(step: "choose" | "pair"): void {
    chooseStep.hidden = step !== "choose";
    pairStep.hidden = step !== "pair";
    addBack.hidden = step === "choose";
    renderPairForm();
  }

  function choose(device: { name: string; host: string; port: number }): void {
    selected = device;
    pairLocked = false;
    pairErrorKey = undefined;
    codeField.value = "";
    if (!accountField.value) accountField.value = accountFilter(overview?.thisDevice.account ?? "");
    showStep("pair");
    (accountField.value ? codeField : accountField).focus();
  }

  function goBack(): void {
    addAttempt += 1;
    pairBusy = false;
    selected = undefined;
    codeField.value = "";
    pairErrorKey = undefined;
    showStep("choose");
    addressField.focus();
  }

  const addOverlay = overlay(addWindow, addClose, () => {
    scanSequence += 1;
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
      remember(`r:${device.id}`, true);
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
    if (parsed && parsed !== "invalid") choose({ name: parsed.host, host: parsed.host, port: parsed.port ?? 22 });
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
  const grant = { control: true, applications: false, actions: false };

  function permissionSwitch(id: keyof typeof grant, iconName: string, label: Message) {
    const input = h("input", { type: "checkbox", switch: true, role: "switch" });
    input.checked = grant[id];
    input.addEventListener("change", () => { grant[id] = input.checked; });
    const row = overlayRow(iconName, label, "label", h("span", { class: "switch" }, input, h("span", { "aria-hidden": "true" })));
    return { root: row.root, input };
  }
  const switches = [
    permissionSwitch("control", "layout-grid", "permControl"),
    permissionSwitch("applications", "app-window", "permApplications"),
    permissionSwitch("actions", "play", "permActions"),
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
    Object.assign(grant, { control: true, applications: false, actions: false });
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
    setupStage.hidden = stage !== "setup";
    openStage.hidden = stage !== "open";
    doneStage.hidden = stage !== "done";
    setText(openLabel, t(opening ? "pairOpening" : "pairOpenFor"));
    if (openButton.disabled !== opening) openButton.disabled = opening;
    openButton.setAttribute("aria-busy", String(opening));
    if (openFailed) openError.set(t("pairOpenFailed"));
    openError.show(openFailed);
    setText(closeLabel, t("pairClosePairing"));
    if (state && stage === "open") {
      const current = shown && shown.expires === state.expires ? shown : undefined;
      setText(codeText, current ? codeFilter(current.code) : "");
      codeText.hidden = !current;
      codeHidden.hidden = Boolean(current);
      setText(accountText, current?.account ?? overview?.thisDevice.account ?? "—");
      const names = [reach?.hostname, ...(reach?.addresses ?? [])].filter((name): name is string => Boolean(name));
      reachText.replaceChildren(...(names.length ? names.map(name => h("span", {}, name)) : [h("span", {}, "—")]));
      setText(timeText, formatSession((state.expires ?? 0) - seconds()));
      setText(attemptsText, String(state.attemptsLeft));
    }
    resultIcon.hidden = outcomeName === undefined;
    setText(resultText, outcomeName === undefined ? t("pairEnded") : fill(t("pairPaired"), { name: outcomeName }));
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
  }

  /** Leaving the view drops what waits for a second click, closes the windows, and drops what was typed in them. */
  function reset(): void {
    closeLog(false);
    closeGuide(false);
    addOverlay.close(false);
    pairOverlay.close(false);
    syncTicker();
    if (confirming) {
      confirming = undefined;
      render();
    }
  }

  function section(key: string, title: Message, fallback = false) {
    const label = h("span", { class: "section-title" }, t(title));
    const summary = h("span", { class: "section-summary" });
    const head = h("div", { class: "section-head" }, chevron(), label, summary);
    const rows = h("div", { class: "section-rows" });
    const body = h("div", { class: "collapse" }, rows);
    disclosure(key, head, label, body, fallback);
    return { root: h("section", { class: "device-section" }, head, body), summary, rows };
  }

  function toolRow(tool: RemoteTool): Row<RemoteTool> {
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
    return {
      root,
      update(current) {
        status.dataset.mode = current.running ? "manual" : "off";
        setText(status, t(current.running ? "manual" : "off"));
        setAttribute(power, "aria-checked", String(current.running));
        setTip(tip, fill(t(current.running ? "toolPowerOff" : "toolPowerOn"), { tool: name }));
      },
    };
  }

  function appRow(app: RemoteApp): Row<RemoteApp> {
    const image = app.icon
      ? h("img", { src: app.icon, alt: "", width: "26", height: "26", draggable: "false" }) : icon("app-window");
    const state = h("span", { class: "device-state" });
    const open = playButton(`${t("open")} ${app.name}`, t("open"));
    const root = h("div", { class: "device-row" },
      h("span", { class: "app-icon" }, image), h("div", { class: "device-row-text" }, h("strong", {}, app.name)), state, open.root);
    return {
      root,
      update(current) {
        state.toggleAttribute("data-on", current.open);
        setText(state, t(current.open ? "stateOpen" : "stateClosed"));
      },
    };
  }

  function moreApps() {
    const label = h("span", {});
    const button = h("button", { type: "button", class: "device-more" }, label, icon("chevron-right"));
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

  function remoteBody(device: RemoteDevice) {
    const key = `r:${device.id}`;
    const toolsSection = section(`${key}/tools`, "tools");
    const appsSection = section(`${key}/apps`, "sectionApplications");
    const actionsSection = section(`${key}/actions`, "sectionActions");
    const shellSection = section(`${key}/shell`, "sectionShell");

    const toolList = list<RemoteTool>(toolsSection.rows, tool => JSON.stringify([host.language(), tool.id]), toolRow);
    const appHost = h("div", {});
    appsSection.rows.append(appHost);
    const appList = list<RemoteApp>(appHost, app => JSON.stringify([host.language(), app.name, app.icon]), appRow);
    const more = moreApps();
    appsSection.rows.append(more.root);
    const actionList = list<SavedAction>(actionsSection.rows, actionSignature, actionRow(false));

    const shellNote = h("span", { class: "setting-note" });
    const terminal = iconButton(t("openTerminal"), "terminal", "outlined");
    shellSection.rows.append(h("div", { class: "device-row" },
      h("span", { class: "action-icon" }, icon("square-terminal")),
      h("div", { class: "device-row-text" }, h("strong", {}, t("openTerminal")), shellNote), terminal.root));

    const offline = h("p", { class: "setting-note device-offline" }, t("deviceOfflineNote"));
    const sections = h("div", { class: "device-sections" },
      toolsSection.root, appsSection.root, actionsSection.root, shellSection.root);
    const root = h("div", { class: "device-body", "data-remote": "" }, sections, offline);

    return {
      root,
      update(current: RemoteDevice): void {
        sections.hidden = current.online !== true;
        offline.hidden = current.online === true;
        const known = current.tools.filter(tool => tools.some(({ id }) => id === tool.id));
        toolList.sync(known);
        setText(toolsSection.summary, fill(t("deviceToolsSummary"), {
          on: known.filter(tool => tool.running).length, total: known.length,
        }));

        const shown = current.apps.pinned.slice(0, appRows);
        appList.sync(shown);
        more.update(Math.max(0, current.apps.total - shown.length));
        setText(appsSection.summary, fill(t("deviceAppsSummary"), { count: current.apps.open }));

        actionList.sync(current.actions);
        const next = current.actions.flatMap(action => action.nextRun === null ? [] : [action.nextRun]);
        setText(actionsSection.summary, next.length
          ? fill(t("deviceActionsNext"), { time: formatClock(Math.min(...next), host.language()) })
          : t(current.actions.length ? "actionManual" : "deviceActionsNone"));

        const on = current.shell !== null && current.shell.until > seconds();
        const until = on ? fill(t("deviceShellOn"), { time: formatClock(current.shell!.until, host.language()) }) : t("off");
        setText(shellSection.summary, until);
        setText(shellNote, on ? until : t("shellOnlyThere"));
        terminal.disable(on ? (desktop.sampleDevices ? undefined : t("notAvailableYet")) : t("shellOnlyThere"), t("openTerminal"));
      },
    };
  }

  function deviceRow(item: Item): Row<Item> {
    const name = h("strong", {}, item.name || t("thisDevice"));
    const state = h("span", { class: "device-state" });
    const open = iconButton(t("openInterface"), "square-arrow-out-up-right", "outlined");
    const forget = iconButton(t("forgetDevice"), "user-minus");
    const remote = item.self ? undefined : item.data as RemoteDevice;
    const { accept, cancel } = confirmButtons(t("forgetAccept"), () => { if (remote) void forgetRemote(remote); }, () => {
      confirming = undefined;
      render();
      forget.button.focus();
    });
    const confirm = h("div", { class: "device-confirm", hidden: true },
      h("span", { class: "setting-note" }, t("forgetConfirm")), accept.root, cancel.root);
    forget.button.addEventListener("click", () => {
      if (!remote) return;
      confirming = `forget:${remote.id}`;
      render();
      cancel.button.focus();
    });
    const head = h("div", { class: "device-head" },
      kindIcon(item.kind),
      h("span", { class: "device-name" }, name, item.self && h("span", { class: "device-badge" }, t("thisDevice"))),
      !item.self && state, !item.self && open.root, !item.self && forget.root, !item.self && confirm, chevron());
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
        const asking = confirming === `forget:${device.id}`;
        state.hidden = asking;
        open.root.hidden = asking;
        forget.root.hidden = asking;
        confirm.hidden = !asking;
        state.toggleAttribute("data-on", device.online === true);
        setText(state, t(device.online === true ? "deviceOnline" : "deviceOffline"));
        open.disable(device.online === true ? (desktop.sampleDevices ? undefined : t("notAvailableYet")) : t("deviceOfflineTip"), t("openInterface"));
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
    if (logWindow.hasAttribute("data-open")) renderLog();
    if (shownLanguage !== host.language()) {
      shownLanguage = host.language();
      retranslate();
    } else {
      renderPairWindow();
    }
    syncTicker();
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
          pairing: { open: false, expires: null, attemptsLeft: 0 }, account: null, controllers: [],
          socket: { listening: true, error: null }, actions: [],
        },
        devices: [],
      };
    }
    render();
  }

  addButton.setAttribute("aria-haspopup", "dialog");
  addButton.addEventListener("click", () => openAdd(addButton));
  addButton.disabled = false;
  host.refreshIcons();

  return { refresh, render, reset };
}
