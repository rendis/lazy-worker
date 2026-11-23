import "./tool-motion.css";
import activityClip from "./assets/clips/activity-loop.mp4";
import activityPoster from "./assets/clips/activity-poster.jpg";
import caffeinateClip from "./assets/clips/caffeinate-loop.mp4";
import caffeinatePoster from "./assets/clips/caffeinate-poster.jpg";
import proxyClip from "./assets/clips/proxy-loop.mp4";
import proxyPoster from "./assets/clips/proxy-poster.jpg";
import type { Preferences, Status } from "./desktop";
import { messages, type Message } from "./messages";

export type ToolId = "activity" | "caffeinate" | "proxy";

type Tool = {
  id: ToolId;
  title: Message;
  description: Message;
  icon: string;
  help: { text: Message; clip?: { video: string; poster: string; label: Message } };
  /** Sidebar entry, when the module sits under a broader section. */
  section?: { title: Message; icon: string };
  /** Tools row, when the module belongs to a group there. */
  row?: { title: Message; group?: string };
};

export const tools: readonly Tool[] = [
  {
    id: "activity", title: "activity", description: "activityDescription", icon: "activity",
    help: { text: "activityExplanation", clip: { video: activityClip, poster: activityPoster, label: "activityClip" } },
  },
  {
    id: "caffeinate", title: "caffeinate", description: "caffeinateDescription", icon: "coffee",
    help: { text: "caffeinateExplanation", clip: { video: caffeinateClip, poster: caffeinatePoster, label: "caffeinateClip" } },
  },
  {
    id: "proxy", title: "proxy", description: "proxyDescription", icon: "server",
    help: { text: "proxyExplanation", clip: { video: proxyClip, poster: proxyPoster, label: "proxyClip" } },
    section: { title: "ai", icon: "sparkles" },
    row: { title: "aiProxy" },
  },
];

const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
const statusModes = ["off", "manual", "scheduled"] as const satisfies readonly Status["mode"][];
const rowAnimations = new WeakMap<HTMLElement, Animation>();
const statusAnimations = new WeakMap<HTMLElement, Animation[]>();

function instantiate(id: string): HTMLElement {
  const template = document.getElementById(id) as HTMLTemplateElement;
  return template.content.firstElementChild!.cloneNode(true) as HTMLElement;
}

function label(node: HTMLElement, message: Message): void {
  node.dataset.text = message;
  node.textContent = messages.en[message];
}

function mountStatus(status: HTMLElement, id: ToolId): void {
  status.dataset.toolStatus = id;
  const content = document.createElement("span");
  content.className = "tool-status-content";
  const current = document.createElement("span");
  current.className = "tool-status-current";
  current.textContent = status.textContent;
  const previous = document.createElement("span");
  previous.className = "tool-status-previous";
  previous.setAttribute("aria-hidden", "true");
  content.append(current, previous);

  // Reserve the widest localized status before changing the live label.
  for (const translations of Object.values(messages)) {
    for (const mode of statusModes) {
      const reserve = document.createElement("span");
      reserve.className = "tool-status-reserve";
      reserve.setAttribute("aria-hidden", "true");
      reserve.textContent = translations[mode];
      content.append(reserve);
    }
  }
  status.replaceChildren(content);
}

export function updateToolStatus(status: HTMLElement, mode: Status["mode"], text: string): void {
  const current = status.querySelector<HTMLElement>(".tool-status-current")!;
  const oldText = current.textContent;
  const modeChanged = status.dataset.mode !== mode;
  if (oldText !== text) current.textContent = text;
  if (!modeChanged) return;

  status.dataset.mode = mode;
  const previous = status.querySelector<HTMLElement>(".tool-status-previous")!;
  for (const animation of statusAnimations.get(status) ?? []) animation.cancel();
  previous.textContent = "";
  if (reducedMotion.matches || !current.animate || status.closest("[hidden]")) return;

  previous.textContent = oldText;
  const timing = { duration: 200, easing: "ease-out" };
  const entering = current.animate([
    { opacity: 0, transform: "translateY(3px) scale(.96)" },
    { opacity: 1, transform: "translateY(0) scale(1)" },
  ], timing);
  const exiting = previous.animate([
    { opacity: 1, visibility: "visible", transform: "translateY(0) scale(1)" },
    { opacity: 0, visibility: "visible", transform: "translateY(-3px) scale(.96)" },
  ], timing);
  const animations = [entering, exiting];
  statusAnimations.set(status, animations);
  exiting.onfinish = () => {
    if (statusAnimations.get(status) !== animations) return;
    previous.textContent = "";
    statusAnimations.delete(status);
  };
}

function mountTool(tool: Tool, navigation: HTMLElement, catalog: HTMLElement): HTMLElement {
  const link = instantiate("tool-nav-template");
  link.dataset.view = tool.id;
  link.querySelector("i")!.dataset.lucide = tool.section?.icon ?? tool.icon;
  label(link.querySelector("span")!, tool.section?.title ?? tool.title);
  navigation.append(link);

  const row = instantiate("tool-card-template");
  const rowLink = row.querySelector<HTMLButtonElement>(".tool-row-link")!;
  const rowTitle = row.querySelector("strong")!;
  const rowSwitchLabel = row.querySelector<HTMLElement>(".catalog-switch-label")!;
  const rowSwitch = row.querySelector("input")!;
  const rowPower = row.querySelector<HTMLButtonElement>("[data-tool-power]")!;
  rowTitle.id = `${tool.id}-row-title`;
  rowSwitchLabel.id = `${tool.id}-row-quick-start`;
  rowLink.dataset.view = tool.id;
  rowLink.setAttribute("aria-labelledby", rowTitle.id);
  rowSwitch.dataset.toolInclude = tool.id;
  rowSwitch.setAttribute("aria-labelledby", `${rowSwitchLabel.id} ${rowTitle.id}`);
  rowPower.dataset.toolPower = tool.id;
  rowPower.setAttribute("aria-labelledby", rowTitle.id);
  row.querySelector<HTMLElement>(".tool-icon")!.dataset.lucide = tool.icon;
  mountStatus(row.querySelector<HTMLElement>("[data-tool-status]")!, tool.id);
  label(rowTitle, tool.row?.title ?? tool.title);
  (tool.row?.group ? document.getElementById(tool.row.group)! : catalog).append(row);
  const pin = row.querySelector<HTMLButtonElement>(".tool-pin")!;
  pin.dataset.toolPin = tool.id;
  pin.setAttribute("aria-labelledby", rowTitle.id);

  const header = instantiate("tool-header-template");
  const heading = header.querySelector("h1")!;
  const power = header.querySelector<HTMLButtonElement>("[data-tool-power]")!;
  const status = header.querySelector<HTMLElement>("[data-tool-status]")!;
  const timer = header.querySelector<HTMLElement>("[data-tool-timer]")!;
  const headerSwitchLabel = header.querySelector<HTMLElement>(".quick-switch-label")!;
  const headerSwitch = header.querySelector("input")!;
  heading.id = `${tool.id}-heading`;
  headerSwitchLabel.id = `${tool.id}-header-quick-start`;
  headerSwitch.dataset.toolInclude = tool.id;
  headerSwitch.setAttribute("aria-labelledby", `${headerSwitchLabel.id} ${heading.id}`);
  power.dataset.toolPower = tool.id;
  power.setAttribute("aria-labelledby", heading.id);
  mountStatus(status, tool.id);
  timer.dataset.toolTimer = tool.id;
  const helpButton = header.querySelector<HTMLButtonElement>("[data-help-button]")!;
  const helpPanel = header.querySelector<HTMLElement>(".help-panel")!;
  const clip = helpPanel.querySelector("video")!;
  helpPanel.id = `${tool.id}-help`;
  helpButton.setAttribute("aria-controls", helpPanel.id);
  label(helpPanel.querySelector("p")!, tool.help.text);
  if (tool.help.clip) {
    clip.src = tool.help.clip.video;
    clip.poster = tool.help.clip.poster;
    clip.dataset.label = tool.help.clip.label;
  } else {
    clip.parentElement!.remove();
    helpPanel.toggleAttribute("data-text-only", true);
  }
  if (tool.id === "activity") {
    power.id = "power";
    status.id = "status";
  } else {
    header.querySelector("[data-permission-warning]")?.remove();
  }
  label(heading, tool.title);
  label(header.querySelector<HTMLElement>(".module-description")!, tool.description);
  document.querySelector(`[data-module-view="${tool.id}"] .module-body`)!.before(header);
  return row;
}

export function updateQuickStartMode(mode: "start" | "stop" | "mixed"): void {
  const actions = document.getElementById("quick-start-actions")!;
  if (actions.dataset.mode === mode) return;

  const start = actions.querySelector<HTMLButtonElement>("#start-selected")!;
  const stop = actions.querySelector<HTMLButtonElement>("#stop-selected")!;
  const outgoing = mode === "start" ? stop : mode === "stop" ? start : undefined;
  for (const button of [start, stop]) {
    if (button === outgoing) continue;
    button.removeAttribute("aria-hidden");
    button.removeAttribute("tabindex");
  }
  actions.dataset.mode = mode;

  if (outgoing) {
    if (document.activeElement === outgoing) {
      const incoming = outgoing === start ? stop : start;
      (incoming.disabled ? actions.closest("[data-module-view]")?.querySelector("h1") : incoming)
        ?.focus({ preventScroll: true });
    }
    outgoing.setAttribute("aria-hidden", "true");
    outgoing.tabIndex = -1;
  }
}

const pinsKey = "lazy-worker-pinned-tools";

/** The tools the user pinned, in the order they were pinned. */
function readPins(): ToolId[] {
  try {
    const saved = JSON.parse(localStorage.getItem(pinsKey) ?? "[]");
    return Array.isArray(saved) ? saved.filter((id): id is ToolId => tools.some(tool => tool.id === id)) : [];
  } catch {
    return [];
  }
}

export function mountTools(): (translate: (message: Message) => string) => void {
  const navigation = document.getElementById("tool-navigation")!;
  const catalog = document.getElementById("tool-catalog")!;
  const rows = tools.map(tool => ({ tool, id: tool.id, row: mountTool(tool, navigation, catalog) }))
    .filter(({ tool }) => !tool.row?.group);
  let currentOrder = rows;
  let pins = readPins();
  let translate: ((message: Message) => string) | undefined;

  function renderPins(): void {
    for (const { id, row } of rows) {
      const pinned = pins.includes(id);
      const button = row.querySelector<HTMLButtonElement>(".tool-pin")!;
      button.setAttribute("aria-pressed", String(pinned));
      row.toggleAttribute("data-pinned", pinned);
      if (translate) {
        const tip = translate(pinned ? "unpinTool" : "pinTool");
        button.parentElement!.dataset.tip = tip;
        button.setAttribute("aria-label", tip);
      }
    }
  }

  for (const { id, row } of rows) {
    row.querySelector(".tool-pin")!.addEventListener("click", () => {
      pins = pins.includes(id) ? pins.filter(pinned => pinned !== id) : [...pins, id];
      try {
        localStorage.setItem(pinsKey, JSON.stringify(pins));
      } catch {
        // Without storage, the pin lasts until the panel reloads.
      }
      renderPins();
      reorder();
    });
  }

  /** Pinned tools come first, in the order they were pinned. The others keep their own order. */
  function reorder(): void {
    const order = [...rows].sort((a, b) => {
      const pa = pins.indexOf(a.id), pb = pins.indexOf(b.id);
      return (pa < 0 ? Infinity : pa) - (pb < 0 ? Infinity : pb) || rows.indexOf(a) - rows.indexOf(b);
    });
    if (order.every((entry, index) => entry === currentOrder[index])) return;
    currentOrder = order;

    const animate = !reducedMotion.matches && !catalog.closest("[hidden]");
    // Use the current on-screen positions when reversing a move in progress.
    const starting = animate ? order.map(({ row }) => row.getBoundingClientRect()) : [];
    const focused = document.activeElement;
    let previous = catalog.firstElementChild!;
    for (const { row } of order) {
      rowAnimations.get(row)?.cancel();
      if (previous.nextElementSibling !== row) previous.after(row);
      previous = row;
    }
    if (focused instanceof HTMLElement && catalog.contains(focused) && document.activeElement !== focused) {
      focused.focus({ preventScroll: true });
    }

    if (!animate) return;
    // Each card glides from its old place to its new one, across rows and columns.
    for (const [index, { row }] of order.entries()) {
      const now = row.getBoundingClientRect();
      const x = starting[index].left - now.left;
      const y = starting[index].top - now.top;
      if ((!x && !y) || !row.animate) continue;
      rowAnimations.set(row, row.animate([
        { transform: `translate(${x}px, ${y}px)` },
        { transform: "translate(0, 0)" },
      ], { duration: 320, easing: "cubic-bezier(.22, .7, .36, 1)" }));
    }
  }

  function updateToolOrder(next: (message: Message) => string): void {
    translate = next;
    renderPins();
    reorder();
  }

  const views = document.querySelectorAll<HTMLElement>("[data-module-view]");
  const links = document.querySelectorAll<HTMLButtonElement>(".sidebar [data-view]");
  const helpButtons = document.querySelectorAll<HTMLButtonElement>("[data-help-button]");
  let openHelp: HTMLButtonElement | undefined;

  function showHelp(button: HTMLButtonElement, visible: boolean): void {
    const panel = document.getElementById(button.getAttribute("aria-controls")!)!;
    panel.toggleAttribute("data-open", visible);
    panel.toggleAttribute("inert", !visible);
    panel.setAttribute("aria-hidden", String(!visible));
    button.setAttribute("aria-expanded", String(visible));
    openHelp = visible ? button : undefined;
    const clip = panel.querySelector("video");
    if (!clip) return;
    if (visible && !reducedMotion.matches) void clip.play().catch(() => {});
    else clip.pause();
    if (!visible) enlargeClip(panel, false);
  }

  function enlargeClip(panel: HTMLElement, enlarged: boolean): void {
    panel.toggleAttribute("data-enlarged", enlarged);
    panel.querySelector(".help-clip")?.setAttribute("aria-pressed", String(enlarged));
  }

  for (const clip of document.querySelectorAll<HTMLButtonElement>(".help-clip")) {
    clip.addEventListener("click", () => {
      const panel = clip.closest<HTMLElement>(".help-panel")!;
      enlargeClip(panel, !panel.hasAttribute("data-enlarged"));
      if (reducedMotion.matches) {
        const video = clip.querySelector("video")!;
        if (video.paused) void video.play().catch(() => {});
        else video.pause();
      }
    });
  }

  for (const button of helpButtons) {
    button.addEventListener("click", () => showHelp(button, openHelp !== button));
  }
  document.addEventListener("keydown", event => {
    if (event.key !== "Escape" || !openHelp) return;
    const button = openHelp;
    showHelp(button, false);
    button.focus();
  });
  function dismissHelpOutside(event: Event): void {
    if (openHelp && !openHelp.parentElement!.contains(event.target as Node)) showHelp(openHelp, false);
  }
  document.addEventListener("pointerdown", dismissHelpOutside);
  document.addEventListener("focusin", dismissHelpOutside);

  function open(view: string, focus: boolean): void {
    if (openHelp) showHelp(openHelp, false);
    for (const section of views) {
      section.toggleAttribute("hidden", section.dataset.moduleView !== view);
      if (focus && !section.hidden) section.querySelector("h1")?.focus();
    }
    for (const link of links) {
      if (link.dataset.view === view) link.setAttribute("aria-current", "page");
      else link.removeAttribute("aria-current");
    }
  }
  for (const button of document.querySelectorAll<HTMLButtonElement>("[data-view]")) {
    button.addEventListener("click", () => open(button.dataset.view!, true));
  }
  open("tools", false);
  return updateToolOrder;
}
