import { desktop } from "./desktop";
import type { Language, Message } from "./messages";
import { h, icon } from "./proxy";
import { formatClock, isToday } from "./time";

// The window that asks before this device lets another one run any command on it.
// Only the person at this device sees it. The name comes from the other device, so it is only ever set as text.

type Host = {
  translate(key: Message): string;
  language(): Language;
  refreshIcons(): void;
  /** Shows the notice of a failed change. `error` is the code that the app sent, when it sent one. */
  fail(error: unknown): void;
  /** Called when the shell of a device changed, so the lists read it again. */
  changed(): void;
};

type Choice = "hour" | "two" | "custom" | "off";

/** The shortest and the longest time that can be granted, in minutes. The app checks them again. */
const MIN_MINUTES = 1;
const MAX_MINUTES = 24 * 60;
/** How often the time that the shell turns off at is worked out again while the window is open. */
const clockTick = 15000;
const presets: Record<"hour" | "two", number> = { hour: 60, two: 120 };
const choices: { id: Choice; label: Message }[] = [
  { id: "hour", label: "shellOneHour" },
  { id: "two", label: "shellTwoHours" },
  { id: "custom", label: "shellCustom" },
  { id: "off", label: "shellUntilOff" },
];

function setText(node: HTMLElement, text: string): void {
  if (node.textContent !== text) node.textContent = text;
}

function fill(text: string, values: Record<string, string | number>): string {
  return text.replace(/\{(\w+)\}/g, (marker, key: string) => key in values ? String(values[key]) : marker);
}

export function mountShellWindow(host: Host) {
  const t = host.translate;
  const labelled: (() => void)[] = [];

  function bind<T extends HTMLElement>(node: T, key: Message, attribute?: string): T {
    const apply = (): void => {
      if (attribute) node.setAttribute(attribute, t(key));
      else setText(node, t(key));
    };
    apply();
    labelled.push(apply);
    return node;
  }

  const shell = document.getElementById("shell-grant-window")!;
  const title = document.getElementById("shell-grant-title")!;
  const body = document.getElementById("shell-grant-body") as HTMLFormElement;
  const close = document.getElementById("shell-grant-close") as HTMLButtonElement;

  let device: { id: string; name: string } | undefined;
  let trigger: HTMLElement | undefined;
  let choice: Choice = "hour";
  let saving = false;
  let ticker: number | undefined;

  const warning = h("p", { class: "shell-warning" });
  const disk = h("p", { class: "shell-warning" });
  const own = bind(h("p", { class: "shell-warning" }), "shellGrantChoice");

  const buttons = choices.map(({ id, label }) => {
    const button = h("button", { type: "button", "aria-pressed": "false" });
    bind(button, label);
    button.addEventListener("click", () => {
      choice = id;
      if (id !== "custom") fields(id === "off" ? undefined : presets[id]);
      render();
      if (id === "custom") hoursField.focus();
    });
    return { id, button };
  });
  const group = h("div", { class: "segments shell-duration", role: "group", style: "--segment-count: 4" },
    ...buttons.map(({ button }) => button), h("span", { class: "segment-indicator", "aria-hidden": "true" }));
  group.setAttribute("aria-label", t("shellDuration"));
  labelled.push(() => group.setAttribute("aria-label", t("shellDuration")));
  const durationRow = h("div", { class: "shell-duration-row" },
    h("span", { class: "setting-label" }, icon("timer"), bind(h("span"), "shellDuration")), group);

  const digits = (text: string, length: number): string => text.replace(/\D/g, "").slice(0, length);
  const hoursField = h("input", {
    class: "field shell-field", type: "text", inputmode: "numeric", maxlength: "2", autocomplete: "off", spellcheck: "false",
  });
  const minutesField = h("input", {
    class: "field shell-field", type: "text", inputmode: "numeric", maxlength: "2", autocomplete: "off", spellcheck: "false",
  });
  bind(hoursField, "shellHoursLabel", "aria-label");
  bind(minutesField, "shellMinutesLabel", "aria-label");
  const endNote = h("span", { class: "setting-note shell-end" });
  const timeRow = h("div", { class: "setting-row shell-time-row" },
    icon("clock"),
    h("div", { class: "shell-time-text" }, bind(h("span", { class: "setting-label" }), "shellTime"), endNote),
    h("div", { class: "device-end shell-time-fields" },
      hoursField, bind(h("span", { class: "shell-unit" }), "shellHoursUnit"),
      minutesField, bind(h("span", { class: "shell-unit" }), "shellMinutesUnit")));
  const logNote = bind(h("p", { class: "setting-note shell-log-note" }), "shellLogNote");
  const cancel = bind(h("button", { type: "button", class: "text-button" }), "cancel");
  const allow = h("button", { type: "submit", class: "access-action" }, icon("terminal"), bind(h("span"), "shellAllow"));
  body.append(warning, disk, own, durationRow, timeRow, logNote,
    h("div", { class: "action-footer" }, h("span", { class: "action-spacer" }), cancel, allow));

  /** Puts a number of minutes in the two fields, or empties them. */
  function fields(minutes: number | undefined): void {
    hoursField.value = minutes === undefined ? "" : String(Math.floor(minutes / 60));
    minutesField.value = minutes === undefined ? "" : String(minutes % 60);
  }

  /** The minutes that the fields say, `null` for no end, and `undefined` for a time that cannot be granted. */
  function chosen(): number | null | undefined {
    if (choice === "off") return null;
    if (hoursField.value === "" && minutesField.value === "") return undefined;
    const total = Number(hoursField.value || "0") * 60 + Number(minutesField.value || "0");
    return Number.isInteger(total) && total >= MIN_MINUTES && total <= MAX_MINUTES && Number(minutesField.value || "0") < 60
      ? total : undefined;
  }

  function render(): void {
    if (!device) return;
    for (const { id, button } of buttons) button.setAttribute("aria-pressed", String(id === choice));
    const custom = choice === "custom";
    for (const field of [hoursField, minutesField]) if (field.disabled !== !custom) field.disabled = !custom;
    const minutes = chosen();
    if (minutes === null) setText(endNote, t("shellEndsNever"));
    else if (minutes === undefined) setText(endNote, t("shellTimeInvalid"));
    else {
      const end = Math.floor(Date.now() / 1000) + minutes * 60;
      setText(endNote, fill(t(isToday(end) ? "shellEndsToday" : "shellEndsLater"), { time: formatClock(end, host.language()) }));
    }
    endNote.toggleAttribute("data-warning", minutes === undefined);
    const ready = minutes !== undefined && !saving;
    if (allow.disabled !== !ready) allow.disabled = !ready;
    setText(title, fill(t("shellGrantTitle"), { name: device.name }));
    setText(warning, fill(t("shellGrantWarning"), { name: device.name }));
    setText(disk, t("shellGrantDisk"));
  }

  function open(next: { id: string; name: string }, from: HTMLElement): void {
    device = next;
    trigger = from;
    saving = false;
    choice = "hour";
    fields(presets.hour);
    render();
    shell.setAttribute("data-open", "");
    shell.inert = false;
    window.clearInterval(ticker);
    ticker = window.setInterval(render, clockTick);
    host.refreshIcons();
    allow.focus();
  }

  function closeWindow(restoreFocus = true): void {
    if (!shell.hasAttribute("data-open")) return;
    shell.removeAttribute("data-open");
    shell.inert = true;
    window.clearInterval(ticker);
    ticker = undefined;
    device = undefined;
    saving = false;
    if (restoreFocus && trigger?.isConnected) trigger.focus();
  }

  for (const [field, length] of [[hoursField, 2], [minutesField, 2]] as const) {
    // A number that is there is replaced by what the person types.
    field.addEventListener("focus", () => field.select());
    field.addEventListener("input", () => {
      const next = digits(field.value, length);
      if (next !== field.value) field.value = next;
      render();
    });
  }
  close.addEventListener("click", () => closeWindow());
  cancel.addEventListener("click", () => closeWindow());
  shell.addEventListener("pointerdown", event => {
    if (event.target === shell) closeWindow();
  });
  body.addEventListener("submit", event => {
    event.preventDefault();
    const minutes = chosen();
    if (!device || saving || minutes === undefined) return;
    saving = true;
    render();
    void desktop.grantShell(device.id, minutes).then(() => {
      closeWindow(false);
      host.changed();
    }, (error: unknown) => {
      saving = false;
      render();
      host.fail(error);
    });
  });

  return {
    open,
    /** Closes the window when it is open. It returns true when it was. */
    closeTop(): boolean {
      if (!shell.hasAttribute("data-open")) return false;
      closeWindow();
      return true;
    },
    closeAll(): void {
      closeWindow(false);
    },
    /** Puts the words of the window in the language of the interface. */
    retranslate(): void {
      for (const apply of labelled) apply();
      render();
    },
  };
}
