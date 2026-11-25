import "./tabs.css";
import { fadeText } from "./motion";

const tabOutMs = 110;
const tabInMs = 180;
const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");

function setAttribute(node: HTMLElement, name: string, value: string): void {
  if (node.getAttribute(name) !== value) node.setAttribute(name, value);
}

export type Tabs = {
  selected(): string;
  select(id: string, focus?: boolean): void;
  /** The small badge after the name of a tab. An empty text hides it. A dot shows before the text: `true` for a state that is on, `"pending"` for one that waits for the person. */
  summary(id: string, text: string, dot?: boolean | "pending"): void;
};

/**
 * Gives a tab bar its behavior. The bar is a `.segments.tab-segments` element with `role="tablist"`. Each tab button has
 * `data-tab` and names its panel through `aria-controls`; a button that is `hidden` when this runs is left out. Every panel has the same height,
 * so a change of tab only fades the panels over each other. The panels keep their elements, so a row in a hidden panel keeps updating.
 */
export function mountTabs(
  bar: HTMLElement, panels: readonly HTMLElement[], options: { initial?: string; onSelect?: (id: string) => void } = {},
): Tabs {
  const entries = [...bar.querySelectorAll<HTMLButtonElement>('[role="tab"]')]
    .filter(button => !button.hidden)
    .map(button => ({
      id: button.dataset.tab!,
      button,
      panel: panels.find(panel => panel.id === button.getAttribute("aria-controls"))!,
      summary: button.querySelector<HTMLElement>(".tab-summary"),
    }));
  const ids = entries.map(entry => entry.id);
  let current = options.initial !== undefined && ids.includes(options.initial) ? options.initial : ids[0];
  bar.style.setProperty("--segment-count", String(entries.length));
  let token = 0;

  const paint = (id: string): void => {
    entries.forEach(({ id: entryId, button }, index) => {
      const selected = entryId === id;
      setAttribute(button, "aria-selected", String(selected));
      setAttribute(button, "tabindex", selected ? "0" : "-1");
      if (selected) bar.style.setProperty("--segment-position", String(index));
    });
    bar.dataset.selection = id;
  };

  /** Shows `to` and hides `from`. The panels have one height, so they only fade over each other. */
  const swap = (from: HTMLElement, to: HTMLElement): void => {
    const mine = ++token;
    const container = to.parentElement!;
    const animate = !reducedMotion.matches && typeof to.animate === "function" && container.isConnected
      && container.offsetHeight > 0 && !container.closest("[hidden]");
    for (const { panel } of entries) {
      for (const animation of panel.getAnimations()) animation.cancel();
      panel.classList.remove("tab-leaving");
      if (panel !== from && panel !== to) panel.hidden = true;
    }
    if (!animate) {
      from.hidden = true;
      to.hidden = false;
      return;
    }
    from.classList.add("tab-leaving");
    to.hidden = false;
    const easing = "cubic-bezier(.22, .7, .36, 1)";
    const out = from.animate([{ opacity: 1 }, { opacity: 0 }], { duration: tabOutMs, easing: "ease-in", fill: "forwards" });
    to.animate([{ opacity: 0 }, { opacity: 1 }], { duration: tabInMs, delay: tabOutMs / 2, easing, fill: "backwards" });
    const end = (): void => {
      if (mine !== token) return;
      from.hidden = true;
      from.classList.remove("tab-leaving");
      out.cancel();
    };
    // A timer also ends it, because a hidden window can pause animations.
    void out.finished.then(end, end);
    window.setTimeout(end, tabOutMs + tabInMs);
  };

  const select = (id: string, focus = false): void => {
    const entry = entries.find(item => item.id === id);
    if (!entry) return;
    if (id !== current) {
      const from = entries.find(item => item.id === current)!.panel;
      current = id;
      paint(id);
      swap(from, entry.panel);
      options.onSelect?.(id);
    }
    if (focus) entry.button.focus();
  };

  paint(current);
  for (const { id, panel } of entries) panel.hidden = id !== current;
  for (const { id, button } of entries) button.addEventListener("click", () => select(id));
  bar.addEventListener("keydown", event => {
    const index = ids.indexOf(current);
    const target = event.key === "ArrowRight" ? (index + 1) % ids.length
      : event.key === "ArrowLeft" ? (index - 1 + ids.length) % ids.length
      : event.key === "Home" ? 0 : event.key === "End" ? ids.length - 1 : -1;
    if (target < 0) return;
    event.preventDefault();
    select(ids[target], true);
  });

  return {
    selected: () => current,
    select,
    summary(id: string, text: string, dot: boolean | "pending" = false): void {
      const summary = entries.find(item => item.id === id)?.summary;
      if (!summary) return;
      summary.toggleAttribute("data-dot", dot !== false);
      summary.toggleAttribute("data-pending", dot === "pending");
      fadeText(summary, text);
    },
  };
}
