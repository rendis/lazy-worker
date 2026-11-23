import {
  desktop, type ProxyApp, type ProxyInstalls, type ProxyLogEntry, type ProxyNetwork, type ProxyPeer,
  type ProxySettings,
} from "./desktop";
import type { Message } from "./messages";

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
  const groupCount = element("ai-group-count");

  let settings: ProxySettings | undefined;
  let serverQuickStart = false;
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

  /** Why a tool cannot open now. The Open button shows it as its tooltip. */
  function blocked(app: ProxyApp): Message | undefined {
    if (!installed(app)) return "proxyNotDetected";
    if (!hasServer()) return "proxyNeedsServer";
  }

  function hasServer(): boolean {
    return serverAddressPattern.test(settings!.server.trim());
  }

  function updateApp(id: string, change: (app: ProxyApp) => ProxyApp): void {
    host.update(current => ({ ...current, apps: current.apps.map(app => app.id === id ? change(app) : app) }));
  }

  async function launch(app: ProxyApp): Promise<void> {
    if (!installed(app)) return;
    if (!hasServer()) {
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

  function age(seconds: number): string {
    return seconds < 60
      ? host.translate("secondsAgo").replace("{n}", String(seconds))
      : host.translate("minutesAgo").replace("{n}", String(Math.floor(seconds / 60)));
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
        : age(peer.idleSeconds)),
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
      h("time", {}, new Date(entry.time * 1000).toLocaleTimeString([], { hour12: false })),
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
    }, ...variantInputs.map(input => h("label", {}, input, h("span", {}, variantLabels[input.value as ProxyApp["variant"]]))),
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
    const hint = h("p", { class: "setting-note proxy-command-hint", hidden: true }, host.translate("proxyCommandHint"));
    const detail = h("div", { class: "collapse", id: `proxy-app-${app.id}-detail`, inert: true },
      h("div", { class: "proxy-app-detail" },
        h("span", { class: "setting-note" }, host.translate("proxyCommand")), code, field, viewLinks, editLinks, hint));
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
        copy.disabled = !hasServer();
        copyTip.dataset.tip = host.translate(hasServer() ? "copyCommand" : "proxyNeedsServer");
        if (setIcon(copy, copiedApp === current.id ? "check" : "copy")) host.refreshIcons();
        disclosure.setAttribute("aria-expanded", String(isOpen));
        detail.toggleAttribute("data-open", isOpen);
        detail.inert = !isOpen;
        code.textContent = resolved(current);
        code.hidden = editing;
        if (!editing) field.value = command(current);
        field.hidden = !editing;
        viewLinks.hidden = editing;
        editLinks.hidden = !editing;
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

  /** Keeps each row's element across updates, so its controls can animate between states. */
  function reconcile(container: HTMLElement, rows: Rows, build: (app: ProxyApp) => Row): void {
    const labels = [host.translate("open"), host.translate("proxyInstalled"), host.quickStartHint()];
    let built = false;
    for (const app of settings!.apps) {
      const signature = JSON.stringify([app.name, app.icon, labels]);
      let entry = rows.get(app.id);
      if (entry?.signature !== signature) {
        const row = build(app);
        entry?.row.root.replaceWith(row.root);
        entry = { signature, row };
        rows.set(app.id, entry);
        built = true;
      }
      entry.row.update(app);
    }
    for (const [id, entry] of rows) {
      if (settings!.apps.every(app => app.id !== id)) {
        entry.row.root.remove();
        rows.delete(id);
      }
    }
    settings!.apps.forEach((app, index) => {
      const root = rows.get(app.id)!.row.root;
      if (container.children[index] !== root) container.insertBefore(root, container.children[index] ?? null);
    });
    if (built) host.refreshIcons();
  }

  function renderApps(): void {
    reconcile(apps, clientRows, clientRow);
    reconcile(groupApps, groupRows, groupRow);
    const marked = Number(serverQuickStart) + settings!.apps.filter(app => app.quickStart).length;
    groupCount.textContent = marked ? host.translate("quickStartMarked").replace("{count}", String(marked)) : "";
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
    }
    if (action === "edit") editingApp = app.id;
    if (action === "cancel") editingApp = undefined;
    if (action === "restore") updateApp(app.id, current => ({ ...current, command: null }));
    if (action === "delete") host.update(current => ({ ...current, apps: current.apps.filter(entry => entry.id !== app.id) }));
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
      command: entered.includes("{server}") ? entered : `${proxyEnvironment} ${entered}`,
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
      serverQuickStart = quickStart;
      running = active;
      view.dataset.role = current.role;
      roles.dataset.selection = current.role;
      for (const input of roleInputs) {
        input.checked = input.value === current.role;
        input.disabled = !enabled;
      }
      roleCaption.textContent = host.translate(current.role === "server" ? "roleServerCaption" : "roleClientCaption");
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
