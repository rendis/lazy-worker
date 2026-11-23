import {
  desktop, type ProxyApp, type ProxyInstalls, type ProxyLogEntry, type ProxyNetwork, type ProxyPeer,
  type ProxySettings,
} from "./desktop";
import type { Message } from "./messages";
import { age, formatMoment, type AgeUnit } from "./time";

// Brand marks from Simple Icons 16.38.1 (CC0). The marks belong to their owners.
const brandFiles = import.meta.glob<string>("./assets/brands/*.svg", { eager: true, import: "default", query: "?url" });
const brandMarks = Object.fromEntries(Object.entries(brandFiles)
  .map(([path, url]) => [path.slice(path.lastIndexOf("/") + 1, -".svg".length), url]));
/** Brands the icon picker offers, with the name each one shows. */
const brandNames: Record<string, string> = {
  claude: "Claude", openai: "OpenAI", googlegemini: "Gemini", deepseek: "DeepSeek", kimi: "Kimi", qwen: "Qwen",
  mistralai: "Mistral", minimax: "MiniMax", meta: "Meta", perplexity: "Perplexity", ollama: "Ollama",
  lmstudio: "LM Studio", openrouter: "OpenRouter", huggingface: "Hugging Face", githubcopilot: "GitHub Copilot",
  cursor: "Cursor", windsurf: "Windsurf", cline: "Cline", opencode: "OpenCode", zedindustries: "Zed",
};
const builtInBrands: Record<string, string> = { claude: "claude", codex: "openai" };

const proxyEnvironment = "HTTPS_PROXY=http://{server} HTTP_PROXY=http://{server} NO_PROXY=localhost,127.0.0.1,::1";
const chromiumProxy = '--args --proxy-server="http://{server}" --proxy-bypass-list="localhost;127.0.0.1;::1"';
// Keep these equal to `src-tauri/src/proxy/apps.rs`.
const builtInCommands: Record<string, Record<ProxyApp["variant"], string>> = {
  claude: {
    app: `open -a Claude ${chromiumProxy}`,
    code: `${proxyEnvironment} claude`,
  },
  codex: {
    app: `open -b com.openai.codex --env HTTPS_PROXY=http://{server} --env HTTP_PROXY=http://{server} --env NO_PROXY=localhost,127.0.0.1,::1 ${chromiumProxy}`,
    code: `${proxyEnvironment} codex`,
  },
};
const variants = ["app", "code"] as const;
const variantLabels = { app: "App", code: "Code" };
const variantIcons = { app: "app-window", code: "square-terminal" };
const variantHints: Record<ProxyApp["variant"], Message> = { app: "variantApp", code: "variantCode" };
const customIcons = [
  "terminal", "sparkles", "bot", "brain", "code", "globe", "cloud", "message-square", "cpu", "rocket", "box",
];
const logOutcomes: Record<ProxyLogEntry["outcome"], Message> = {
  ok: "logOk", denied: "logDenied", blocked: "logBlocked", unreachable: "logUnreachable", invalid: "logInvalid",
};
const listPreview = 3;
const maxAllowed = 64;
type WindowMode = "log" | "access" | "peers";
const windowTitles: Record<WindowMode, Message> = { log: "proxyLog", access: "proxyAccess", peers: "proxyPeers" };
const serverAddressPattern = /^[A-Za-z0-9.\-[\]:]+:\d{1,5}$/;
const ageMessages: Record<AgeUnit, Message> = {
  seconds: "secondsAgo", minutes: "minutesAgo", hours: "hoursAgo", days: "daysAgo",
};
const ipv4 = /^(?:(?:25[0-5]|2[0-4]\d|1?\d?\d)\.){3}(?:25[0-5]|2[0-4]\d|1?\d?\d)$/;

type Host = {
  translate(key: Message): string;
  quickStartHint(): string;
  update(apply: (settings: ProxySettings) => ProxySettings): void;
  fail(message: Message): void;
  refreshIcons(): void;
};
type Child = Node | string | false | undefined;

function h<K extends keyof HTMLElementTagNameMap>(
  tag: K, attributes: Record<string, string | boolean | undefined> = {}, ...children: Child[]
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  for (const [name, value] of Object.entries(attributes)) {
    if (value === true) node.setAttribute(name, "");
    else if (value !== false && value !== undefined) node.setAttribute(name, value);
  }
  node.append(...children.filter((child): child is Node | string => Boolean(child)));
  return node;
}

function icon(name: string): HTMLElement {
  return h("i", { "data-lucide": name });
}

/** A brand mark when the name is one, and a line icon otherwise. */
function anyIcon(name: string): HTMLElement {
  const mark = brandMarks[name];
  if (!mark) return icon(name);
  const node = h("span", { class: "brand-icon", "aria-hidden": "true" });
  node.style.setProperty("--brand", `url("${mark}")`);
  return node;
}

/** A built-in tool keeps its own brand. Any other tool shows the icon the user chose. */
function appIcon(app: ProxyApp): HTMLElement {
  return anyIcon(builtInBrands[app.id] ?? app.icon);
}

export function mountProxy(host: Host) {
  const element = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
  const view = document.querySelector<HTMLElement>('[data-module-view="proxy"]')!;
  const roles = element("proxy-roles");
  const roleInputs = roles.querySelectorAll<HTMLInputElement>("input");
  const roleCaption = element("proxy-role-caption");
  const serverPanel = element("proxy-server");
  const clientPanel = element("proxy-client");
  const addresses = element("proxy-addresses");
  const port = element<HTMLInputElement>("proxy-port");
  const accessNote = element("proxy-access-note");
  const allowed = element("proxy-allowed");
  const allowForm = element<HTMLFormElement>("proxy-allow-form");
  const allow = element<HTMLInputElement>("proxy-allow");
  const peerList = element("proxy-peers");
  const logCount = element("proxy-log-count");
  const logToggle = element<HTMLButtonElement>("proxy-log-toggle");
  const logPanel = element("proxy-log-window");
  const logTip = element("proxy-log-tip");
  const logWindowCount = element("proxy-log-window-count");
  const logClose = element<HTMLButtonElement>("proxy-log-close");
  const windowTitle = element("proxy-log-title");
  const windowNote = element("proxy-window-note");
  const logList = element("proxy-log");
  const serverAddress = element<HTMLInputElement>("proxy-server-address");
  const serverState = element("proxy-server-state");
  const apps = element("proxy-apps");
  const addButton = element<HTMLButtonElement>("proxy-add-button");
  const addForm = element<HTMLFormElement>("proxy-add-form");
  const addIconButton = element<HTMLButtonElement>("proxy-add-icon");
  const addIcons = element("proxy-add-icons");
  const addName = element<HTMLInputElement>("proxy-add-name");
  const addCommand = element<HTMLInputElement>("proxy-add-command");
  const groupToggle = element<HTMLButtonElement>("ai-group-toggle");
  const groupBody = element("ai-group-body");
  const groupApps = element("ai-group-apps");
  // The proxy's own row heads the group. Its arrow takes the place of the row's arrow to the module.
  const groupHead = document.querySelector<HTMLElement>('#tool-catalog [data-tool-power="proxy"]')!.closest<HTMLElement>(".tool-row")!;
  const groupToggleTip = groupToggle.parentElement!;
  groupHead.querySelector(".tool-arrow")?.replaceWith(groupToggleTip);

  let settings: ProxySettings | undefined;
  let running = false;
  let network: ProxyNetwork = { hostname: null, addresses: [] };
  let peers: ProxyPeer[] = [];
  let log: ProxyLogEntry[] = [];
  let windowMode: WindowMode | undefined;
  let windowTrigger: HTMLElement | undefined;
  let allowInvalid = false;
  let peersRequested = 0;
  let installs: ProxyInstalls | undefined;
  let reachable: boolean | undefined;
  let checkedAddress: string | undefined;
  let openApp: string | undefined;
  let editingApp: string | undefined;
  let launchedApp: string | undefined;
  let copiedApp: string | undefined;
  let deletingApp: string | undefined;
  let serverRequested = false;
  let copied: string | undefined;
  let addIcon = customIcons[0];
  let addressDelay: number | undefined;
  const signatures: Record<string, string> = {};

  function sync(region: string, state: unknown, build: () => void): void {
    const signature = JSON.stringify(state);
    if (signatures[region] === signature) return;
    signatures[region] = signature;
    build();
    host.refreshIcons();
  }

  function title(app: ProxyApp): string {
    return app.id in builtInCommands ? `${app.name} ${variantLabels[app.variant]}` : app.name;
  }

  function defaultCommand(app: ProxyApp): string | undefined {
    return builtInCommands[app.id]?.[app.variant];
  }

  function command(app: ProxyApp): string {
    return app.command ?? defaultCommand(app) ?? "";
  }

  function resolved(app: ProxyApp): string {
    return command(app).split("{server}").join(settings!.server.trim() || "{server}");
  }

  function installed(app: ProxyApp): boolean {
    if (app.id === "claude") return Boolean(app.variant === "app" ? installs?.claudeApp : installs?.claudeCode);
    if (app.id === "codex") return Boolean(app.variant === "app" ? installs?.codexApp : installs?.codexCode);
    return true;
  }

  /** A command needs the server address only when it names it. */
  function needsServer(app: ProxyApp): boolean {
    return command(app).includes("{server}") && !hasServer();
  }

  /** Why a tool cannot open now. The Open button shows it as its tooltip. */
  function blocked(app: ProxyApp): Message | undefined {
    if (!installed(app)) return "proxyNotDetected";
    if (needsServer(app)) return "proxyNeedsServer";
  }

  function hasServer(): boolean {
    return serverAddressPattern.test(settings!.server.trim());
  }

  function updateApp(id: string, change: (app: ProxyApp) => ProxyApp): void {
    host.update(current => ({ ...current, apps: current.apps.map(app => app.id === id ? change(app) : app) }));
  }

  async function launch(app: ProxyApp): Promise<void> {
    if (!installed(app)) return;
    if (needsServer(app)) {
      // Start selected reaches this point for a marked tool without a server address.
      serverRequested = true;
      host.fail("proxyServerMissing");
      renderLists();
      return;
    }
    try {
      await desktop.launchProxyApp(app.id);
      launchedApp = app.id;
      window.setTimeout(() => {
        if (launchedApp === app.id) launchedApp = undefined;
        renderLists();
      }, 1400);
    } catch {
      host.fail("proxyLaunchFailed");
    }
    renderLists();
  }

  function elapsed(seconds: number): string {
    const { value, unit } = age(seconds);
    return host.translate(ageMessages[unit]).replace("{n}", String(value));
  }

  type Row = { root: HTMLElement; update(app: ProxyApp): void };
  type Rows = Map<string, { signature: string; row: Row }>;
  const clientRows: Rows = new Map();
  const groupRows: Rows = new Map();

  function quickSwitch(app: ProxyApp): { root: HTMLElement; input: HTMLInputElement } {
    const input = h("input", {
      type: "checkbox", role: "switch", "data-quick-start": true, "data-action": "quick", "data-app": app.id,
      "aria-label": `${host.translate("quickStart")} ${title(app)}`,
    });
    return {
      input,
      root: h("label", { class: "switch catalog-switch", "data-quick-start-hint": host.quickStartHint() },
        input, h("span", { "aria-hidden": "true" })),
    };
  }

  /** The wrapper carries the tooltip, because a disabled button receives no pointer events. */
  function openButton(app: ProxyApp, compact: boolean) {
    const button = h("button", {
      type: "button", class: compact ? "round-action" : "open-button", "data-action": "open", "data-app": app.id,
    }, icon("play"), !compact && h("span", {}, host.translate("open")));
    const root = h("span", { class: "tip" }, button);
    return {
      root,
      button,
      update(current: ProxyApp): void {
        const reason = blocked(current);
        button.disabled = reason !== undefined;
        button.setAttribute("aria-label", `${host.translate("open")} ${title(current)}`);
        if (reason) {
          root.dataset.tip = host.translate(reason);
          button.setAttribute("aria-description", host.translate(reason));
        } else {
          delete root.dataset.tip;
          button.removeAttribute("aria-description");
        }
        if (setIcon(button, launchedApp === current.id ? "check" : "play")) host.refreshIcons();
      },
    };
  }

  /** Swaps a button's icon only when it changes, so a steady state keeps its element. */
  function setIcon(button: HTMLElement, name: string): boolean {
    if ((button.dataset.icon ?? "play") === name) return false;
    button.dataset.icon = name;
    button.firstElementChild!.replaceWith(icon(name));
    return true;
  }

  function tipButton(action: string, app: ProxyApp, name: string, label: Message, variant = ""): HTMLButtonElement {
    return h("button", {
      type: "button", class: `icon-button tip ${variant}`.trim(), "data-action": action, "data-app": app.id,
      "data-tip": host.translate(label), "aria-label": host.translate(label),
    }, icon(name));
  }

  function renderAddresses(): void {
    const targets = [network.hostname, ...network.addresses].filter((host): host is string => Boolean(host))
      .map(host => `${host}:${settings!.port}`);
    sync("addresses", [targets, copied, host.translate("copy")], () => {
      addresses.replaceChildren(...(targets.length ? targets.map(target => h("div", { class: "proxy-address" },
        h("code", {}, target),
        h("button", {
          type: "button", class: "icon-button", "data-copy": target, "aria-label": `${host.translate("copy")} ${target}`,
        }, icon(copied === target ? "check" : "copy")),
      )) : [h("span", { class: "setting-note" }, host.translate("proxyNoAddress"))]));
    });
  }

  function moreButton(mode: WindowMode, hidden: number): HTMLElement {
    return h("button", { type: "button", class: "text-link", "data-window": mode },
      host.translate("moreCount").replace("{count}", String(hidden)));
  }

  function allowedItem(address: string): HTMLElement {
    return h("li", {},
      h("span", {}, address),
      h("button", {
        type: "button", "data-remove": address,
        "aria-label": host.translate("removeAddress").replace("{address}", address),
      }, icon("x")));
  }

  function peerItem(peer: ProxyPeer): HTMLElement {
    const active = peer.connections > 0;
    const allowed = settings!.allowed.includes(peer.address);
    // The server always admits its own computer, so that one needs no entry.
    const allowable = ipv4.test(peer.address) && !peer.address.startsWith("127.");
    const label = host.translate(allowed ? "proxyAllowed" : "proxyAllowPeer");
    return h("li", { class: "proxy-peer", "data-active": active },
      h("span", { class: "proxy-peer-name" }, peer.name ?? peer.address),
      peer.name ? h("span", { class: "setting-note" }, peer.address) : false,
      h("span", { class: "setting-note proxy-peer-activity" }, active
        ? host.translate("proxyPeerActive").replace("{count}", String(peer.connections))
        : elapsed(peer.idleSeconds)),
      allowable && h("span", { class: "tip", "data-tip": label },
        h("button", {
          type: "button", class: "icon-button", "data-allow": peer.address, "aria-label": `${label} ${peer.address}`,
          disabled: allowed || settings!.allowed.length >= maxAllowed,
        }, icon(allowed ? "check" : "plus"))));
  }

  function renderAccess(): void {
    const list = settings!.allowed;
    accessNote.textContent = host.translate(
      allowInvalid ? "proxyInvalidAddress" : list.length ? "proxyAccessLimited" : "proxyAccessOpen");
    accessNote.toggleAttribute("data-warning", allowInvalid || list.length === 0);
    if (allowInvalid) allow.setAttribute("aria-invalid", "true");
    else allow.removeAttribute("aria-invalid");
    sync("allowed", [list, host.translate("moreCount")], () => {
      allowed.replaceChildren(...list.slice(0, listPreview).map(allowedItem),
        list.length > listPreview && h("li", { class: "list-more" }, moreButton("access", list.length - listPreview)) || "");
    });
  }

  function renderPeers(): void {
    sync("peers", [running, peers, settings!.allowed, host.translate("proxyPeerIdle")], () => {
      if (!running || peers.length === 0) {
        peerList.replaceChildren(h("li", { class: "setting-note" },
          host.translate(running ? "proxyPeersNone" : "proxyPeersOff")));
        return;
      }
      peerList.replaceChildren(...peers.slice(0, listPreview).map(peerItem),
        peers.length > listPreview && h("li", { class: "list-more" }, moreButton("peers", peers.length - listPreview)) || "");
    });
  }

  function logItem(entry: ProxyLogEntry): HTMLElement {
    return h("li", { class: "proxy-log-entry", "data-outcome": entry.outcome },
      h("time", {}, formatMoment(entry.time, document.documentElement.lang)),
      h("span", {}, entry.peer),
      h("span", { class: "proxy-log-target" }, entry.target || "—"),
      h("span", { class: "proxy-log-outcome" }, host.translate(logOutcomes[entry.outcome])));
  }

  /** One window over the view lists what a row cannot hold: requests, allowed addresses, or computers. */
  function renderWindow(): void {
    const count = !running ? "" : host.translate("proxyLogCount").replace("{count}", String(log.length));
    logCount.textContent = count;
    logTip.dataset.tip = host.translate("proxyLogShow");
    const mode = windowMode;
    if (!mode) return;
    const items = { log, access: settings!.allowed, peers }[mode];
    windowTitle.textContent = host.translate(windowTitles[mode]);
    logWindowCount.textContent = mode === "log" ? count : String(items.length);
    windowNote.textContent = mode === "log" ? host.translate("proxyLogNote")
      : mode === "access" ? host.translate(items.length ? "proxyAccessLimited" : "proxyAccessOpen") : "";
    windowNote.hidden = windowNote.textContent === "";
    sync("window", [mode, running, items, settings!.allowed, host.translate("logOk")], () => {
      logList.dataset.mode = mode;
      if (items.length === 0) {
        const empty: Message = mode === "access" ? "proxyAccessOpen"
          : mode === "peers" ? (running ? "proxyPeersNone" : "proxyPeersOff") : running ? "proxyLogEmpty" : "proxyLogOff";
        logList.replaceChildren(h("li", { class: "setting-note" }, host.translate(empty)));
        return;
      }
      logList.replaceChildren(...(mode === "log" ? log.map(logItem)
        : mode === "access" ? settings!.allowed.map(allowedItem) : peers.map(peerItem)));
    });
  }

  function clientRow(app: ProxyApp): Row {
    const custom = defaultCommand(app) === undefined;
    const variantInputs = custom ? [] : variants.map(value =>
      h("input", { type: "radio", name: `proxy-variant-${app.id}`, value, "data-action": "variant", "data-app": app.id }));
    const variant = variantInputs.length > 0 && h("div", {
      class: "segments radio-segments mini-segments", role: "radiogroup", "aria-label": app.name,
    }, ...variantInputs.map(input => {
      const value = input.value as ProxyApp["variant"];
      const hint = host.translate(variantHints[value]);
      input.setAttribute("aria-label", hint);
      return h("label", { class: "tip", "data-tip": hint }, input, h("span", {}, icon(variantIcons[value])));
    }),
    h("span", { class: "segment-indicator", "aria-hidden": "true" }));
    const state = h("span", { class: "proxy-app-state" });
    const quick = quickSwitch(app);
    const open = openButton(app, false);
    const copy = h("button", {
      type: "button", class: "icon-button", "data-action": "copy", "data-app": app.id,
      "aria-label": host.translate("copyCommand"),
    }, icon("copy"));
    const copyTip = h("span", { class: "tip" }, copy);
    const code = h("code");
    const field = h("input", {
      class: "field proxy-command-field", type: "text", "data-command": app.id, hidden: true,
      "aria-label": host.translate("proxyCommand"), autocomplete: "off", spellcheck: "false",
    });
    const restore = !custom && tipButton("restore", app, "rotate-ccw", "restore");
    const viewLinks = h("span", { class: "proxy-app-links" },
      tipButton("edit", app, "pencil", "edit"), restore, custom && tipButton("delete", app, "trash-2", "remove", "danger"));
    const editLinks = h("span", { class: "proxy-app-links", hidden: true },
      tipButton("save", app, "check", "save", "confirm"), tipButton("cancel", app, "x", "cancel"));
    // Deleting asks first, in the same place, because it cannot be undone.
    const confirmLinks = h("span", { class: "proxy-app-links proxy-delete-confirm", hidden: true },
      h("span", { class: "setting-note" }, host.translate("proxyDeleteConfirm")),
      tipButton("confirm-delete", app, "trash-2", "remove", "danger"), tipButton("cancel", app, "x", "cancel"));
    const hint = h("p", { class: "setting-note proxy-command-hint", hidden: true }, host.translate("proxyCommandHint"));
    const detail = h("div", { class: "collapse", id: `proxy-app-${app.id}-detail`, inert: true },
      h("div", { class: "proxy-app-detail" },
        h("span", { class: "setting-note" }, host.translate("proxyCommand")), code, field, viewLinks, editLinks,
        confirmLinks, hint));
    const disclosure = h("button", {
      type: "button", class: "icon-button disclosure", "data-action": "toggle", "data-app": app.id,
      "aria-controls": detail.id, "aria-label": host.translate("showCommand").replace("{app}", title(app)),
    }, icon("chevron-right"));
    const root = h("div", { class: "proxy-app" },
      h("div", { class: "setting-row proxy-app-row" },
        appIcon(app), h("span", { class: "setting-label" }, app.name),
        h("span", { class: "proxy-app-meta" }, variant, state), quick.root, copyTip, open.root, disclosure),
      detail);

    return {
      root,
      update(current) {
        const present = installed(current);
        const isOpen = openApp === current.id;
        const editing = isOpen && editingApp === current.id;
        root.toggleAttribute("data-missing", !present);
        if (variant) variant.dataset.selection = current.variant;
        for (const input of variantInputs) input.checked = input.value === current.variant;
        state.hidden = !installs || custom;
        if (state.dataset.present !== String(present)) {
          state.dataset.present = String(present);
          state.replaceChildren(...[present && icon("check"),
            host.translate(present ? "proxyInstalled" : "proxyMissing")].filter(Boolean) as (Node | string)[]);
          host.refreshIcons();
        }
        quick.input.checked = current.quickStart;
        quick.input.disabled = !present;
        open.update(current);
        copy.disabled = needsServer(current);
        copyTip.dataset.tip = host.translate(needsServer(current) ? "proxyNeedsServer" : "copyCommand");
        if (setIcon(copy, copiedApp === current.id ? "check" : "copy")) host.refreshIcons();
        disclosure.setAttribute("aria-expanded", String(isOpen));
        detail.toggleAttribute("data-open", isOpen);
        detail.inert = !isOpen;
        code.textContent = resolved(current);
        code.hidden = editing;
        if (!editing) field.value = command(current);
        field.hidden = !editing;
        const deleting = isOpen && deletingApp === current.id;
        viewLinks.hidden = editing || deleting;
        editLinks.hidden = !editing;
        confirmLinks.hidden = !deleting;
        hint.hidden = !editing;
        if (restore) restore.disabled = current.command === null;
      },
    };
  }

  function groupRow(app: ProxyApp): Row {
    const name = h("strong");
    const state = h("span", { class: "tool-app-state" });
    const quick = quickSwitch(app);
    const open = openButton(app, true);
    const root = h("div", { class: "tool-row tool-app-row" },
      h("span", { class: "tool-name" }, appIcon(app), name),
      state, quick.root, open.root, h("span", {}));
    return {
      root,
      update(current) {
        const present = installed(current);
        root.toggleAttribute("data-missing", !present);
        name.textContent = title(current);
        state.textContent = host.translate(present || !installs ? "roleClient" : "proxyMissing");
        quick.input.checked = current.quickStart;
        quick.input.disabled = !present;
        open.update(current);
      },
    };
  }

  const rowMotion = { duration: 240, easing: "cubic-bezier(.22, .7, .36, 1)" };
  const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");

  /** Opens a new row from nothing, so the rows under it move down instead of jumping. */
  function enter(root: HTMLElement): void {
    if (reducedMotion.matches || !root.animate) return;
    const height = `${root.offsetHeight}px`;
    root.style.overflow = "hidden";
    settle(root.animate([{ height: "0px", opacity: 0 }, { height, opacity: 1 }], rowMotion),
      () => { root.style.overflow = ""; });
  }

  /** A hidden window can pause animations, so a timer also ends the change. */
  function settle(animation: Animation, done: () => void): void {
    void animation.finished.catch(() => {}).then(done);
    window.setTimeout(() => {
      animation.cancel();
      done();
    }, rowMotion.duration + 80);
  }

  /** Closes a removed row before it leaves, so the rows under it move up instead of jumping. */
  function leave(root: HTMLElement): void {
    if (reducedMotion.matches || !root.animate) {
      root.remove();
      return;
    }
    root.toggleAttribute("data-leaving", true);
    root.inert = true;
    root.style.overflow = "hidden";
    settle(root.animate([{ height: `${root.offsetHeight}px`, opacity: 1 }, { height: "0px", opacity: 0 }], rowMotion),
      () => root.remove());
  }

  /** Keeps each row's element across updates, so its controls can animate between states. */
  function reconcile(container: HTMLElement, rows: Rows, build: (app: ProxyApp) => Row): void {
    const first = rows.size === 0;
    const added: HTMLElement[] = [];
    const labels = [host.translate("open"), host.translate("proxyInstalled"), host.translate("variantApp"), host.quickStartHint()];
    let built = false;
    for (const app of settings!.apps) {
      const signature = JSON.stringify([app.name, app.icon, labels]);
      let entry = rows.get(app.id);
      if (entry?.signature !== signature) {
        const row = build(app);
        if (entry) entry.row.root.replaceWith(row.root);
        else if (!first) added.push(row.root);
        entry = { signature, row };
        rows.set(app.id, entry);
        built = true;
      }
      entry.row.update(app);
    }
    for (const [id, entry] of rows) {
      if (settings!.apps.every(app => app.id !== id)) {
        leave(entry.row.root);
        rows.delete(id);
      }
    }
    // A row that is closing keeps its place and does not count for the order.
    settings!.apps.forEach((app, index) => {
      const root = rows.get(app.id)!.row.root;
      const placed = [...container.children].filter(child => !child.hasAttribute("data-leaving"));
      if (placed[index] !== root) container.insertBefore(root, placed[index] ?? null);
    });
    if (built) host.refreshIcons();
    added.forEach(enter);
  }

  function renderApps(): void {
    reconcile(apps, clientRows, clientRow);
    reconcile(groupApps, groupRows, groupRow);
  }

  function renderServerState(): void {
    const address = settings!.server.trim();
    if (hasServer()) serverRequested = false;
    const key: Message = !address ? "proxyServerEmpty" : !hasServer() ? "proxyServerNoPort"
      : reachable === undefined ? "proxyServerChecking" : reachable ? "proxyServerAvailable" : "proxyServerUnreachable";
    serverState.textContent = host.translate(key);
    serverState.dataset.tone = !hasServer() ? (serverRequested || address ? "error" : "idle")
      : reachable === undefined ? "idle" : reachable ? "ok" : "error";
  }

  function renderLists(): void {
    if (!settings) return;
    renderAddresses();
    renderAccess();
    renderPeers();
    renderWindow();
    renderApps();
    renderServerState();
  }

  function checkServer(force = false): void {
    const address = settings?.server.trim() ?? "";
    if (!force && address === checkedAddress) return;
    checkedAddress = address;
    reachable = undefined;
    if (!address) return;
    void desktop.checkProxyServer(address).catch(() => false).then(result => {
      if (checkedAddress !== address) return;
      reachable = result;
      renderLists();
    });
  }

  function refreshPeers(): void {
    if (!running) {
      peers = [];
      log = [];
      return;
    }
    if (view.hidden || settings?.role !== "server" || performance.now() - peersRequested < 900) return;
    peersRequested = performance.now();
    void desktop.getProxyPeers().catch(() => []).then(current => {
      peers = current;
      renderLists();
    });
    void desktop.getProxyLog().catch(() => []).then(current => {
      log = current;
      renderLists();
    });
  }

  function refreshEnvironment(): void {
    void desktop.getProxyNetwork().catch(() => ({ hostname: null, addresses: [] })).then(current => {
      network = current;
      renderLists();
    });
    void desktop.detectProxyApps().catch(() => ({ claudeCode: false, claudeApp: false, codexCode: false, codexApp: false })).then(current => {
      installs = current;
      renderLists();
    });
    checkServer(true);
  }

  function findApp(target: EventTarget | null): { action: string; app: ProxyApp } | undefined {
    const control = target instanceof Element ? target.closest<HTMLElement>("[data-action]") : null;
    const app = settings?.apps.find(entry => entry.id === (control?.dataset.app ?? "claude"));
    return control && app ? { action: control.dataset.action!, app } : undefined;
  }

  function act(event: Event): void {
    const found = findApp(event.target);
    if (!found) return;
    const { action, app } = found;
    const control = event.target as HTMLInputElement;
    // A switch or radio reports its new value in `change`. Rendering on its click would undo it first.
    if (event.type === "click" && (action === "quick" || action === "variant")) return;
    if (event.type === "change") {
      if (action === "quick") updateApp(app.id, current => ({ ...current, quickStart: control.checked }));
      // The saved command belongs to the variant it was written for.
      if (action === "variant") {
        updateApp(app.id, current => ({ ...current, variant: control.value as ProxyApp["variant"], command: null }));
      }
      return;
    }
    if (action === "open") void launch(app);
    if (action === "copy") {
      void navigator.clipboard.writeText(resolved(app)).then(() => {
        copiedApp = app.id;
        renderLists();
        window.setTimeout(() => {
          if (copiedApp === app.id) copiedApp = undefined;
          renderLists();
        }, 1400);
      }).catch(() => {});
    }
    if (action === "toggle") {
      openApp = openApp === app.id ? undefined : app.id;
      editingApp = undefined;
      deletingApp = undefined;
    }
    if (action === "edit") editingApp = app.id;
    if (action === "cancel") {
      editingApp = undefined;
      deletingApp = undefined;
    }
    if (action === "restore") updateApp(app.id, current => ({ ...current, command: null }));
    if (action === "delete") deletingApp = app.id;
    if (action === "confirm-delete") {
      deletingApp = undefined;
      host.update(current => ({ ...current, apps: current.apps.filter(entry => entry.id !== app.id) }));
    }
    if (action === "save") {
      const value = apps.querySelector<HTMLInputElement>(`[data-command="${app.id}"]`)!.value.trim();
      if (!value) return;
      updateApp(app.id, current => ({ ...current, command: value === defaultCommand(app) ? null : value }));
      editingApp = undefined;
    }
    renderLists();
    if (action === "edit") apps.querySelector<HTMLInputElement>(`[data-command="${app.id}"]`)?.focus();
  }

  function setAdding(adding: boolean): void {
    addButton.hidden = adding;
    addForm.hidden = !adding;
    addIcons.hidden = true;
    addIconButton.setAttribute("aria-expanded", "false");
    if (adding) {
      addName.value = "";
      addCommand.value = "";
      chooseIcon(customIcons[0]);
      addName.focus();
    }
  }

  function chooseIcon(name: string): void {
    addIcon = name;
    addIconButton.replaceChildren(anyIcon(name));
    for (const option of addIcons.querySelectorAll("button")) {
      option.setAttribute("aria-pressed", String(option.dataset.icon === name));
    }
    host.refreshIcons();
  }

  addIcons.replaceChildren(...[...customIcons, ...Object.keys(brandNames).filter(name => name in brandMarks)].map(name =>
    h("button", {
      type: "button", class: brandNames[name] ? "tip" : undefined, "data-icon": name, "data-tip": brandNames[name],
      "aria-label": brandNames[name] ?? name, "aria-pressed": "false",
    }, anyIcon(name))));
  for (const input of roleInputs) {
    input.addEventListener("change", () => {
      if (input.checked) host.update(current => ({ ...current, role: input.value as ProxySettings["role"] }));
    });
  }
  port.addEventListener("change", () => {
    const value = port.valueAsNumber;
    if (Number.isInteger(value) && value >= 1024 && value <= 65535) host.update(current => ({ ...current, port: value }));
    else port.value = String(settings?.port ?? "");
  });
  allowForm.addEventListener("submit", event => {
    event.preventDefault();
    const address = allow.value.trim();
    allowInvalid = !ipv4.test(address);
    if (!allowInvalid) {
      allowAddress(address);
      allow.value = "";
    }
    renderLists();
  });
  // Only digits and dots can form an IPv4 address, so nothing else reaches the field.
  allow.addEventListener("input", () => {
    const clean = allow.value.replace(/[^0-9.]/g, "").slice(0, 15);
    if (clean !== allow.value) allow.value = clean;
    allowInvalid = false;
    renderLists();
  });
  addresses.addEventListener("click", event => {
    const target = (event.target as Element).closest<HTMLElement>("[data-copy]")?.dataset.copy;
    if (!target) return;
    void navigator.clipboard.writeText(target).then(() => {
      copied = target;
      renderLists();
      window.setTimeout(() => {
        if (copied === target) copied = undefined;
        renderLists();
      }, 1400);
    }).catch(() => {});
  });
  function saveServerAddress(): void {
    if (addressDelay === undefined) return;
    window.clearTimeout(addressDelay);
    addressDelay = undefined;
    const server = serverAddress.value.trim();
    host.update(current => ({ ...current, server }));
  }
  serverAddress.addEventListener("input", () => {
    window.clearTimeout(addressDelay);
    addressDelay = window.setTimeout(saveServerAddress, 500);
  });
  serverAddress.addEventListener("change", saveServerAddress);
  for (const container of [apps, groupApps]) {
    container.addEventListener("click", act);
    container.addEventListener("change", act);
  }
  apps.addEventListener("keydown", event => {
    const field = event.target instanceof HTMLInputElement && event.target.dataset.command;
    if (!field) return;
    if (event.key === "Enter") apps.querySelector<HTMLElement>(`[data-action="save"][data-app="${field}"]`)?.click();
    if (event.key === "Escape") {
      event.stopPropagation();
      apps.querySelector<HTMLElement>(`[data-action="cancel"][data-app="${field}"]`)?.click();
    }
  });
  // Leaving the view discards unsaved input, so it does not reappear later.
  new MutationObserver(() => {
    if (!view.hidden) return;
    setLogOpen(false);
    setAdding(false);
    editingApp = undefined;
    deletingApp = undefined;
    renderLists();
  }).observe(view, { attributes: true, attributeFilter: ["hidden"] });
  document.addEventListener("pointerdown", event => {
    if (addIcons.hidden || (event.target instanceof Node && addIconButton.parentElement!.contains(event.target))) return;
    addIcons.hidden = true;
    addIconButton.setAttribute("aria-expanded", "false");
  });
  addButton.addEventListener("click", () => setAdding(true));
  addIconButton.addEventListener("click", () => {
    addIcons.hidden = !addIcons.hidden;
    addIconButton.setAttribute("aria-expanded", String(!addIcons.hidden));
  });
  addIcons.addEventListener("click", event => {
    const name = (event.target as Element).closest<HTMLElement>("[data-icon]")?.dataset.icon;
    if (!name) return;
    chooseIcon(name);
    addIcons.hidden = true;
    addIconButton.setAttribute("aria-expanded", "false");
  });
  addForm.addEventListener("reset", () => window.setTimeout(() => setAdding(false)));
  addForm.addEventListener("keydown", event => {
    if (event.key === "Escape") setAdding(false);
  });
  addForm.addEventListener("submit", event => {
    event.preventDefault();
    const name = addName.value.trim();
    const entered = addCommand.value.trim();
    if (!name || !entered) return;
    const app: ProxyApp = {
      id: `custom-${Date.now().toString(36)}`,
      name,
      icon: addIcon,
      command: entered,
      quickStart: false,
      variant: "code",
    };
    host.update(current => ({ ...current, apps: [...current.apps, app] }));
    setAdding(false);
  });
  function setWindow(mode: WindowMode | undefined, trigger?: HTMLElement): void {
    if (windowMode === mode) return;
    const previous = windowTrigger;
    windowMode = mode;
    windowTrigger = mode ? trigger : undefined;
    logPanel.toggleAttribute("data-open", mode !== undefined);
    logPanel.inert = mode === undefined;
    logToggle.setAttribute("aria-expanded", String(mode === "log"));
    renderLists();
    if (mode) logClose.focus();
    else if (!view.hidden && previous?.isConnected) previous.focus();
  }
  const setLogOpen = (open: boolean) => setWindow(open ? "log" : undefined, logToggle);
  /** Rows and the window share these actions, so a list behaves the same in both places. */
  function listAction(event: Event): void {
    const control = (event.target as Element).closest<HTMLElement>("[data-remove], [data-allow], [data-window]");
    if (!control) return;
    const { remove, allow: address, window: mode } = control.dataset;
    if (remove) host.update(current => ({ ...current, allowed: current.allowed.filter(entry => entry !== remove) }));
    if (address) allowAddress(address);
    if (mode) setWindow(mode as WindowMode, control);
  }
  function allowAddress(address: string): void {
    host.update(current => current.allowed.includes(address) || current.allowed.length >= maxAllowed
      ? current : { ...current, allowed: [...current.allowed, address] });
  }
  serverPanel.addEventListener("click", listAction);
  logList.addEventListener("click", listAction);
  logToggle.addEventListener("click", () => setLogOpen(true));
  logClose.addEventListener("click", () => setLogOpen(false));
  logPanel.addEventListener("pointerdown", event => {
    if (event.target === logPanel) setLogOpen(false);
  });
  document.addEventListener("keydown", event => {
    if (event.key === "Escape") setLogOpen(false);
  });
  // The server's row in Tools leads to the server, whatever role was showing last.
  groupHead.addEventListener("click", event => {
    if (!(event.target as Element).closest(".tool-row-link") || settings?.role === "server") return;
    host.update(current => ({ ...current, role: "server" }));
  });
  groupToggle.addEventListener("click", () => {
    const expanded = groupToggle.getAttribute("aria-expanded") !== "true";
    groupToggle.setAttribute("aria-expanded", String(expanded));
    groupBody.toggleAttribute("data-open", expanded);
    groupBody.inert = !expanded;
  });
  refreshEnvironment();

  return {
    refresh: refreshEnvironment,
    render(current: ProxySettings, quickStart: boolean, active: boolean, enabled: boolean): void {
      settings = current;
      running = active;
      view.dataset.role = current.role;
      roles.dataset.selection = current.role;
      for (const input of roleInputs) {
        input.checked = input.value === current.role;
        input.disabled = !enabled;
      }
      roleCaption.textContent = host.translate(current.role === "server" ? "roleServerCaption" : "roleClientCaption");
      addName.placeholder = host.translate("proxyName");
      addCommand.placeholder = host.translate("proxyCommand");
      serverPanel.hidden = current.role !== "server";
      clientPanel.hidden = current.role !== "client";
      for (const field of [port, allow, serverAddress, addButton]) field.disabled = !enabled;
      if (document.activeElement !== port) port.value = String(current.port);
      if (document.activeElement !== serverAddress && addressDelay === undefined) serverAddress.value = current.server;
      checkServer();
      refreshPeers();
      renderLists();
    },
    hasQuickStart(): boolean {
      return Boolean(settings?.apps.some(app => app.quickStart && installed(app)));
    },
    async launchQuickStart(): Promise<void> {
      for (const app of settings?.apps ?? []) {
        if (app.quickStart) await launch(app);
      }
    },
  };
}
