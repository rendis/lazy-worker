import "./tool-motion.css";
import type { Preferences, Status } from "./desktop";
import { messages, type Message } from "./messages";

type Tool = {
  id: string;
  title: Message;
  description: Message;
  icon: string;
};

export const tools = [
  { id: "activity", title: "activity", description: "activityDescription", icon: "activity" },
  { id: "caffeinate", title: "caffeinate", description: "caffeinateDescription", icon: "coffee" },
] as const satisfies readonly Tool[];

export type ToolId = (typeof tools)[number]["id"];

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

function mountTool(tool: (typeof tools)[number], navigation: HTMLElement, catalog: HTMLElement): HTMLElement {
  const link = instantiate("tool-nav-template");
  link.dataset.view = tool.id;
  link.querySelector("i")!.dataset.lucide = tool.icon;
  label(link.querySelector("span")!, tool.title);
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
  label(rowTitle, tool.title);
  catalog.append(row);

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
  if (tool.id === "activity") {
    power.id = "power";
    status.id = "status";
  } else {
    header.querySelector("[data-permission-warning]")?.remove();
  }
  header.querySelector<HTMLElement>(".tool-icon")!.dataset.lucide = tool.icon;
  label(heading, tool.title);
  label(header.querySelector("p")!, tool.description);
  document.querySelector(`[data-module-view="${tool.id}"]`)!.prepend(header);
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

export function mountTools(): (quickStart: Preferences["quickStart"]) => void {
  const navigation = document.getElementById("tool-navigation")!;
  const catalog = document.getElementById("tool-catalog")!;
  const rows = tools.map(tool => ({ id: tool.id, row: mountTool(tool, navigation, catalog) }));
  let currentOrder = rows;

  function updateToolOrder(quickStart: Preferences["quickStart"]): void {
    const order = [...rows].sort((a, b) => Number(quickStart[b.id]) - Number(quickStart[a.id]));
    if (order.every((entry, index) => entry === currentOrder[index])) return;
    currentOrder = order;

    const animate = !reducedMotion.matches && !catalog.closest("[hidden]");
    // Use the current on-screen positions when reversing a move in progress.
    const startingTops = animate ? order.map(({ row }) => row.getBoundingClientRect().top) : [];
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
    const offsets = order.map(({ row }, index) => startingTops[index] - row.getBoundingClientRect().top);
    for (const [index, { row }] of order.entries()) {
      const offset = offsets[index];
      if (!offset || !row.animate) continue;
      rowAnimations.set(row, row.animate([
        { transform: `translateY(${offset}px)` },
        { transform: "translateY(0)" },
      ], { duration: 260, easing: "cubic-bezier(.22, .7, .36, 1)" }));
    }
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
