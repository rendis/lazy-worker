import { desktop, type AppNotification } from "./desktop";
import { messages, type Language, type Message } from "./messages";
import { age, type AgeUnit } from "./time";

const ageMessages: Record<Exclude<AgeUnit, "seconds">, Message> = {
  minutes: "minutesAgo", hours: "hoursAgo", days: "daysAgo",
};
const kindIcons: Record<AppNotification["kind"], string> = {
  message: "inbox", error: "triangle-alert", update: "download", info: "info",
};
/** Characters to reserve for the age of a row. Its longest text is `hace 99 min`. */
export const BELL_AGE_WIDTH = 11;

function fill(text: string, values: Record<string, string | number>): string {
  return text.replace(/\{(\w+)\}/g, (match, name: string) => (name in values ? String(values[name]) : match));
}

function isMessage(key: string): key is Message {
  return Object.prototype.hasOwnProperty.call(messages.en, key);
}

type Row = { root: HTMLLIElement; button: HTMLButtonElement; title: HTMLElement; detail: HTMLElement; time: HTMLElement; entry: AppNotification };

/** The bell in the title bar and its window. Text that a device sent is set as text only. */
export function mountBell({ translate, language, refreshIcons, navigate }: {
  translate: (message: Message) => string;
  language: () => Language;
  refreshIcons: () => void;
  /** Opens a place of the panel, such as a view name. */
  navigate: (route: string) => void;
}) {
  const tip = document.querySelector<HTMLElement>(".bell-tip")!;
  const button = document.getElementById("bell-button") as HTMLButtonElement;
  const count = document.getElementById("bell-count")!;
  const overlay = document.getElementById("bell-window")!;
  const list = document.getElementById("bell-list")!;
  const empty = document.getElementById("bell-empty")!;
  const close = document.getElementById("bell-close") as HTMLButtonElement;
  const readAll = document.getElementById("bell-read-all") as HTMLButtonElement;
  const idle = document.getElementById("bell-footer-idle")!;
  const confirm = document.getElementById("bell-footer-confirm")!;
  const clear = document.getElementById("bell-clear") as HTMLButtonElement;
  const clearCancel = document.getElementById("bell-clear-cancel") as HTMLButtonElement;
  const clearYes = document.getElementById("bell-clear-yes") as HTMLButtonElement;
  const rows = new Map<string, Row>();
  let entries: AppNotification[] = [];
  let renderedLanguage: Language | undefined;
  let revision = 0;

  tip.hidden = false;

  function title(entry: AppNotification): string {
    return isMessage(entry.key) ? fill(translate(entry.key), entry.params) : entry.key;
  }

  function detail(entry: AppNotification): string {
    const key = entry.params.detail;
    return key && isMessage(key) ? fill(translate(key), entry.params) : "";
  }

  function elapsed(at: number): string {
    const { value, unit } = age(Date.now() / 1000 - at);
    return unit === "seconds" ? translate("justNow") : fill(translate(ageMessages[unit]), { n: value });
  }

  function build(entry: AppNotification): Row {
    const root = document.createElement("li");
    const rowButton = document.createElement("button");
    rowButton.type = "button";
    rowButton.className = "bell-row";
    const kind = document.createElement("span");
    kind.className = "bell-kind";
    kind.dataset.kind = entry.kind;
    const icon = document.createElement("i");
    icon.dataset.lucide = kindIcons[entry.kind];
    kind.append(icon);
    const text = document.createElement("span");
    text.className = "bell-text";
    const titleNode = document.createElement("strong");
    const detailNode = document.createElement("span");
    text.append(titleNode, detailNode);
    const time = document.createElement("span");
    time.className = "bell-age";
    const dot = document.createElement("span");
    dot.className = "bell-dot";
    dot.setAttribute("aria-hidden", "true");
    rowButton.append(kind, text, dot, time);
    rowButton.addEventListener("click", () => {
      const current = rows.get(entry.id)?.entry ?? entry;
      if (!current.read) void desktop.markNotificationsRead([current.id]).catch(() => {});
      setOpen(false);
      if (current.route && current.route !== "bell") navigate(current.route);
    });
    root.append(rowButton);
    return { root, button: rowButton, title: titleNode, detail: detailNode, time, entry };
  }

  function update(row: Row, entry: AppNotification): void {
    row.entry = entry;
    row.title.textContent = title(entry);
    row.detail.textContent = detail(entry);
    row.detail.hidden = row.detail.textContent === "";
    row.time.textContent = elapsed(entry.at);
    row.root.toggleAttribute("data-unread", !entry.read);
  }

  function renderCount(): void {
    const unread = entries.filter(entry => !entry.read).length;
    count.hidden = unread === 0;
    count.textContent = unread > 99 ? "99+" : String(unread);
    button.setAttribute("aria-label", unread ? fill(translate("notificationsUnread"), { n: unread }) : translate("notifications"));
    readAll.disabled = unread === 0;
    clear.disabled = entries.length === 0;
  }

  function render(): void {
    const languageChanged = renderedLanguage !== language();
    renderedLanguage = language();
    let added = false;
    const shown = entries.map(entry => {
      let row = rows.get(entry.id);
      if (!row) {
        row = build(entry);
        rows.set(entry.id, row);
        added = true;
      }
      update(row, entry);
      return row.root;
    });
    for (const [id, row] of rows) {
      if (!entries.some(entry => entry.id === id)) {
        row.root.remove();
        rows.delete(id);
      }
    }
    list.replaceChildren(...shown);
    empty.hidden = entries.length > 0;
    renderCount();
    if (added || languageChanged) refreshIcons();
  }

  async function refresh(): Promise<void> {
    const current = ++revision;
    try {
      const next = await desktop.listNotifications();
      if (current !== revision) return;
      entries = next;
      render();
    } catch {
      // The bell keeps what it showed. The next change reads it again.
    }
  }

  function showConfirm(open: boolean): void {
    idle.hidden = open;
    confirm.hidden = !open;
    (open ? clearCancel : clear).focus();
  }

  function setOpen(open: boolean): void {
    if (overlay.hasAttribute("data-open") === open) return;
    overlay.toggleAttribute("data-open", open);
    overlay.inert = !open;
    button.setAttribute("aria-expanded", String(open));
    if (open) {
      idle.hidden = false;
      confirm.hidden = true;
      // The ages move on while the window is closed.
      for (const row of rows.values()) row.time.textContent = elapsed(row.entry.at);
      list.scrollTop = 0;
      close.focus();
    } else {
      button.focus();
    }
  }

  button.addEventListener("click", () => {
    setOpen(true);
    void refresh();
  });
  close.addEventListener("click", () => setOpen(false));
  overlay.addEventListener("click", event => { if (event.target === overlay) setOpen(false); });
  overlay.addEventListener("keydown", event => { if (event.key === "Escape") setOpen(false); });
  readAll.addEventListener("click", () => void desktop.markNotificationsRead().catch(() => {}));
  clear.addEventListener("click", () => showConfirm(true));
  clearCancel.addEventListener("click", () => showConfirm(false));
  clearYes.addEventListener("click", () => {
    showConfirm(false);
    void desktop.clearNotifications().catch(() => {});
  });
  void desktop.onNotificationsChanged(() => void refresh());
  void refresh();

  return {
    render,
    refresh,
    open(): void {
      setOpen(true);
      void refresh();
    },
  };
}
