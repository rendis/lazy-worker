import "./devices.css";
import {
  desktop, type ActionSchedule, type Controller, type ControllerPermission, type DeviceKind, type DevicesOverview,
  type RemoteApp, type RemoteDevice, type RemoteTool, type SavedAction,
} from "./desktop";
import type { Language, Message } from "./messages";
import { tools } from "./modules";
import { h, icon, type Child } from "./proxy";
import { age, formatClock, type AgeUnit } from "./time";

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
const kindIcons: Record<DeviceKind, string> = { laptop: "laptop", desktop: "monitor" };
const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");

type Host = {
  translate(key: Message): string;
  language(): Language;
  refreshIcons(): void;
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

  let overview: DevicesOverview | undefined;
  let requested = 0;
  let opened = readOpen();
  let built = false;
  let sequence = 0;

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
  function note(text: string) {
    const paragraph = h("p", { class: "setting-note" }, text);
    const root = h("div", { class: "collapse", inert: true }, h("div", {}, paragraph));
    return {
      root,
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

  function controllerRow(controller: Controller): Row<Controller> {
    const detail = h("span", { class: "setting-note" });
    const chips = permissions.map(permission => {
      const state = h("span", { class: "visually-hidden" });
      const chip = h("span", { class: "permission-chip" },
        icon(permission.icon), h("span", { "aria-hidden": "true" }, t(permission.label)), state);
      return { permission, chip, state };
    });
    const remove = iconButton(fill(t("removeController"), { device: controller.name }), "user-minus");
    const root = h("div", { class: "device-row" },
      kindIcon(controller.kind),
      h("div", { class: "device-row-text" }, h("strong", {}, controller.name), detail),
      h("div", { class: "permission-chips" }, ...chips.map(({ chip }) => chip)),
      remove.root);
    return {
      root,
      update(current) {
        setText(detail, current.lastAccess === null ? t("noAccessYet") : fill(t("lastAccess"), { age: elapsed(current.lastAccess) }));
        for (const { permission, chip, state } of chips) {
          const on = current.permissions[permission.id] === true;
          chip.toggleAttribute("data-on", on);
          setText(state, fill(t(on ? "permissionOn" : "permissionOff"), { permission: t(permission.label) }));
        }
      },
    };
  }

  function settingRow(name: string, label: Message, ...end: Child[]) {
    const text = h("span", { class: "setting-label" }, t(label));
    return { text, root: h("div", { class: "setting-row" }, icon(name), text, h("div", { class: "device-end" }, ...end)) };
  }

  function thisDeviceBody() {
    const remoteState = h("span", { class: "device-state" });
    const access = settingRow("shield-check", "remoteAccess", remoteState);
    const pairingState = h("span", { class: "setting-note" });
    const pairing = iconButton(t("openPairing"), "lock-keyhole-open", "outlined");
    const pairingRow = settingRow("lock-keyhole", "pairing", pairingState, pairing.root);

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
    const cut = h("span", { class: "tip device-cut-tip" }, cutButton);

    const root = h("div", { class: "device-body" },
      access.root, pairingRow.root,
      h("div", { class: "device-subhead" }, h("h3", {}, t("whoCanControl"))),
      controllerRows, controllersEmpty.root,
      h("div", { class: "device-subhead" }, h("h3", {}, t("deviceActions")), addAction.root),
      actionRows, actionsEmpty.root,
      logRow.root, cut);

    return {
      root,
      update(data: DevicesOverview["thisDevice"]): void {
        const accessKey: Record<typeof data.remoteAccess, Message> = {
          unknown: "remoteAccessUnknown", off: "remoteAccessOff", on: "remoteAccessOn",
        };
        setText(remoteState, t(accessKey[data.remoteAccess] ?? "remoteAccessUnknown"));
        remoteState.toggleAttribute("data-on", data.remoteAccess === "on");
        setText(pairingState, t(data.pairing === "open" ? "pairingOpen" : "pairingClosed"));
        // Pairing, actions, and the log arrive in later steps, so the app shows them off until then.
        const later = desktop.sampleDevices ? undefined : t("notAvailableYet");
        pairing.disable(data.pairing === "open" ? t("pairingOpen") : later, t("openPairing"));
        addAction.disable(later, t("addAction"));
        log.disable(later, t("showLog"));
        controllerList.sync(data.controllers);
        controllersEmpty.show(data.controllers.length === 0);
        actionList.sync(data.actions);
        actionsEmpty.show(data.actions.length === 0);
        const nobody = data.controllers.length === 0;
        if (cutButton.disabled !== nobody) cutButton.disabled = nobody;
        if (nobody) setTip(cut, t("noControllers"));
        else cut.removeAttribute("data-tip");
      },
    };
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
        sections.hidden = !current.online;
        offline.hidden = current.online;
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
    const head = h("div", { class: "device-head" },
      kindIcon(item.kind),
      h("span", { class: "device-name" }, name, item.self && h("span", { class: "device-badge" }, t("thisDevice"))),
      !item.self && state, !item.self && open.root, chevron());
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
        state.toggleAttribute("data-on", device.online);
        setText(state, t(device.online ? "deviceOnline" : "deviceOffline"));
        open.disable(device.online ? (desktop.sampleDevices ? undefined : t("notAvailableYet")) : t("deviceOfflineTip"), t("openInterface"));
        (content as ReturnType<typeof remoteBody>).update(device);
      },
    };
  }

  const devices = list<Item>(deviceList, item => JSON.stringify([host.language(), item.self, item.name, item.kind]), deviceRow);

  function render(): void {
    if (!overview) return;
    const { thisDevice } = overview;
    // Adding a device arrives in a later step. The preview shows the button as it will look.
    addButton.disabled = !desktop.sampleDevices;
    if (desktop.sampleDevices) addTip.removeAttribute("data-tip");
    else setTip(addTip, t("notAvailableYet"));
    devices.sync([
      { id: "this", self: true, name: thisDevice.name, kind: thisDevice.kind, data: thisDevice },
      ...overview.devices.map(device => ({
        id: `r:${device.id}`, self: false, name: device.name, kind: device.kind, data: device,
      })),
    ]);
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
        thisDevice: { name: "", kind: "desktop", remoteAccess: "unknown", pairing: "closed", controllers: [], actions: [] },
        devices: [],
      };
    }
    render();
  }

  return { refresh, render };
}
