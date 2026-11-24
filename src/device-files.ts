import { desktop } from "./desktop";
import type { FilesMode, FilesScope, PrivacyCheck, PrivacyTouch } from "./files";
import type { Language, Message } from "./messages";
import { h, icon } from "./proxy";
import { fadeText } from "./motion";
import { formatClock, isToday } from "./time";

// The window that asks which files another device may use on this one, and for how long.
// Only the person at this device sees it. The device name comes from the other device, and the folder names from the
// system, so they are only ever set as text.

type Host = {
  translate(key: Message): string;
  language(): Language;
  refreshIcons(): void;
  /** Shows the notice of a failed change. `error` is the code that the app sent, when it sent one. */
  fail(error: unknown): void;
  /** Called when the files of a device changed, so the lists read it again. */
  changed(): void;
  /** The home folder of this device, as the app reads it, or `null` when it is not known. */
  home(): string | null;
};

type Choice = "hour" | "two" | "custom" | "off";

/** The shortest and the longest time that can be granted, in minutes. The app checks them again. */
const MIN_MINUTES = 1;
const MAX_MINUTES = 24 * 60;
/** The most folders that can be shared. The app checks it again. */
const MAX_FOLDERS = 16;
/** The folder that is offered first. The app creates it when it is chosen and does not exist yet. */
const SUGGESTED = "~/Lazy Worker";
/** How often the time that the sharing ends at is worked out again while the window is open. */
const clockTick = 15000;
const presets: Record<"hour" | "two", number> = { hour: 60, two: 120 };
const choices: { id: Choice; label: Message; icon: string }[] = [
  { id: "hour", label: "shellOneHour", icon: "timer" },
  { id: "two", label: "shellTwoHours", icon: "hourglass" },
  { id: "custom", label: "shellCustom", icon: "pencil" },
  { id: "off", label: "shellUntilOff", icon: "infinity" },
];
const scopes: { id: FilesScope; label: Message; icon: string }[] = [
  { id: "folders", label: "filesScopeFolders", icon: "folder" },
  { id: "home", label: "filesScopeHome", icon: "house" },
  { id: "disk", label: "filesScopeDisk", icon: "hard-drive" },
];
/** The places that macOS asks about, by the path that the app sends. Any other path shows as it is. */
const places: Record<string, Message> = {
  "~/Desktop": "filesPlaceDesktop",
  "~/Documents": "filesPlaceDocuments",
  "~/Downloads": "filesPlaceDownloads",
  "~/Library/Mobile Documents": "filesPlaceICloud",
};
const touches: Record<PrivacyTouch, { icon: string; label: Message }> = {
  readable: { icon: "circle-check", label: "filesTouchReadable" },
  denied: { icon: "circle-x", label: "filesTouchDenied" },
  pending: { icon: "clock", label: "filesTouchPending" },
  failed: { icon: "triangle-alert", label: "filesTouchFailed" },
};
/** How long a save may take before the window says that macOS may ask questions. A quick save never shows it. */
const PROGRESS_DELAY = 400;
const modes: { id: FilesMode; label: Message; icon: string }[] = [
  { id: "read", label: "filesModeRead", icon: "eye" },
  { id: "readWrite", label: "filesModeWrite", icon: "pencil" },
];

function setText(node: HTMLElement, text: string): void {
  if (node.textContent !== text) node.textContent = text;
}

function fill(text: string, values: Record<string, string | number>): string {
  return text.replace(/\{(\w+)\}/g, (marker, key: string) => key in values ? String(values[key]) : marker);
}

export function mountFilesGrantWindow(host: Host) {
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

  /** The info icon of an option: a few plain words, shown on hover or keyboard focus. */
  function info(key: Message): HTMLElement {
    const node = h("span", { class: "tip option-info", tabindex: "0", role: "img" }, icon("info"));
    const apply = (): void => {
      node.setAttribute("data-tip", t(key));
      node.setAttribute("aria-label", t(key));
    };
    apply();
    labelled.push(apply);
    return node;
  }

  /** A choice that shows each option with an icon beside its word. */
  function choice<T extends string>(items: { id: T; label: Message; icon: string }[], label: Message, onPick: (id: T) => void) {
    const buttons = items.map(({ id, label: key, icon: name }) => {
      const button = h("button", { type: "button", "aria-pressed": "false" }, icon(name), bind(h("span"), key));
      button.addEventListener("click", () => onPick(id));
      return { id, button };
    });
    const group = h("div", { class: "segments segment-icons files-choice", role: "group", style: `--segment-count: ${items.length}` },
      ...buttons.map(({ button }) => button), h("span", { class: "segment-indicator", "aria-hidden": "true" }));
    bind(group, label, "aria-label");
    return {
      group,
      select(selected: T): void {
        for (const { id, button } of buttons) button.setAttribute("aria-pressed", String(id === selected));
      },
    };
  }

  /** Content that opens and closes over a short time, and cannot be reached while closed. */
  function collapse(...children: HTMLElement[]) {
    const root = h("div", { class: "collapse", inert: true }, h("div", {}, ...children));
    return {
      root,
      set(open: boolean): void {
        root.toggleAttribute("data-open", open);
        root.toggleAttribute("inert", !open);
      },
    };
  }

  const shell = document.getElementById("files-grant-window")!;
  const title = document.getElementById("files-grant-title")!;
  const body = document.getElementById("files-grant-body") as HTMLFormElement;
  const close = document.getElementById("files-grant-close") as HTMLButtonElement;

  let device: { id: string; name: string } | undefined;
  let trigger: HTMLElement | undefined;
  let scope: FilesScope = "folders";
  let mode: FilesMode = "read";
  let duration: Choice = "hour";
  let folders: string[] = [SUGGESTED];
  let saving = false;
  let picking = false;
  let ticker: number | undefined;
  /** `form` asks, `wait` is macOS asking its questions, and `result` says which places can be read. */
  let step: "form" | "wait" | "result" = "form";
  let checks: PrivacyCheck[] = [];
  /** The scope that was saved, so the result step knows whether to recommend Full Disk Access. */
  let saved: FilesScope = "folders";
  /** Ends the answer of an earlier save when the window closed or opened again. */
  let session = 0;

  // Where: the folders that the person picks, or the whole disk.
  const scopeChoice = choice(scopes, "filesScope", id => {
    scope = id;
    render();
  });
  const scopeRow = h("div", { class: "shell-duration-row" },
    h("span", { class: "setting-label" }, icon("folder-key"), bind(h("span"), "filesScope"), info("filesScopeHelp")), scopeChoice.group);

  const list = h("ul", { class: "files-folders" });
  const add = h("button", { type: "button", class: "files-add" }, icon("plus"), bind(h("span"), "filesAddFolder"));
  const foldersNote = h("p", { class: "setting-note files-note", "data-warning": true });
  const foldersNoteBox = collapse(foldersNote);
  const suggestedNote = bind(h("p", { class: "setting-note files-note" }), "filesSuggestedNote");
  const foldersBox = collapse(
    h("div", { class: "files-folders-box" }, list, add),
    foldersNoteBox.root, suggestedNote);

  // The home folder and the whole disk open a lot, so each says plainly what it reaches.
  // The first sentence says what is shared, in bold. The warning is stronger than a note: it has its own tint and an alert icon.
  const warningLead = h("strong");
  const warningRest = h("span");
  const warning = h("p", { class: "shell-warning files-disk-warning", role: "alert" }, icon("triangle-alert"), h("span", { class: "files-warning-text" }, warningLead, warningRest));
  // The exact folder that the home choice shares, as the device reads it.
  const homePath = h("code", { class: "files-home-path-text" });
  const homeNote = h("p", { class: "setting-note files-note files-home-path" }, icon("house"), bind(h("span"), "filesHomePath"), homePath);
  const homeBox = collapse(homeNote);
  const hidden = bind(h("p", { class: "setting-note files-note" }), "filesAlwaysHidden");
  const wideBox = collapse(warning, hidden);

  // Permission: read only, or read and write.
  const modeChoice = choice(modes, "filesMode", id => {
    mode = id;
    render();
  });
  const modeRow = h("div", { class: "shell-duration-row" },
    h("span", { class: "setting-label" }, icon("shield-check"), bind(h("span"), "filesMode"), info("filesModeHelp")), modeChoice.group);
  const allows = h("p", { class: "shell-warning files-allows" });
  const allowsBox = collapse(allows);

  // Duration, as the window of the shell asks it.
  const durationChoice = choice(choices, "shellDuration", id => {
    duration = id;
    if (id !== "custom") fields(id === "off" ? undefined : presets[id]);
    render();
    if (id === "custom") hoursField.focus();
  });
  const durationRow = h("div", { class: "shell-duration-row" },
    h("span", { class: "setting-label" }, icon("timer"), bind(h("span"), "shellDuration")), durationChoice.group);
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
  const logNote = bind(h("p", { class: "setting-note shell-log-note" }), "filesLogNote");
  const ahead = bind(h("p", { class: "setting-note shell-log-note" }), "filesPrivacyAhead");
  const cancel = bind(h("button", { type: "button", class: "text-button" }), "cancel");
  const allow = h("button", { type: "submit", class: "access-action" }, icon("folder-key"), bind(h("span"), "filesAllow"));
  const formStep = h("div", { class: "files-step" }, scopeRow, foldersBox.root, homeBox.root, wideBox.root, modeRow, allowsBox.root, durationRow, timeRow, logNote, ahead,
    h("div", { class: "action-footer" }, h("span", { class: "action-spacer" }), cancel, allow));

  // After the grant is saved, macOS asks about the protected folders. The person is at this device, so the questions come now.
  const progress = h("p", { class: "files-progress" }, icon("loader-circle"), bind(h("span"), "filesPrivacyAsk"));
  const progressBox = collapse(progress);
  const privacyList = h("ul", { class: "files-privacy-list" });
  const privacySummary = h("p", { class: "setting-note files-note" });
  const fdaNote = bind(h("p", { class: "setting-note files-note" }), "filesFullDiskTip");
  const fdaButton = h("button", { type: "button", class: "access-action files-fda" }, icon("shield-check"), bind(h("span"), "filesFullDiskOpen"));
  const fdaBox = collapse(fdaNote, h("div", { class: "files-fda-row" }, fdaButton));
  const done = h("button", { type: "button", class: "access-action" }, icon("check"), bind(h("span"), "filesDone"));
  const privacyStep = h("div", { class: "files-step", hidden: true }, progressBox.root, privacyList, privacySummary, fdaBox.root,
    h("div", { class: "action-footer" }, h("span", { class: "action-spacer" }), done));
  body.append(formStep, privacyStep);

  /** Puts a number of minutes in the two fields, or empties them. */
  function fields(minutes: number | undefined): void {
    hoursField.value = minutes === undefined ? "" : String(Math.floor(minutes / 60));
    minutesField.value = minutes === undefined ? "" : String(minutes % 60);
  }

  /** The minutes that the fields say, `null` for no end, and `undefined` for a time that cannot be granted. */
  function chosen(): number | null | undefined {
    if (duration === "off") return null;
    if (hoursField.value === "" && minutesField.value === "") return undefined;
    const total = Number(hoursField.value || "0") * 60 + Number(minutesField.value || "0");
    return Number.isInteger(total) && total >= MIN_MINUTES && total <= MAX_MINUTES && Number(minutesField.value || "0") < 60
      ? total : undefined;
  }

  function renderFolders(): void {
    const rows = folders.map(folder => {
      const remove = h("button", { type: "button", class: "icon-button", "aria-label": fill(t("filesRemoveFolder"), { folder }) }, icon("circle-minus"));
      remove.addEventListener("click", () => {
        folders = folders.filter(item => item !== folder);
        render();
        add.focus();
      });
      const row = h("li", { class: "files-folder" },
        icon("folder"), h("span", { class: "files-folder-path" }, folder),
        h("span", { class: "tip", "data-tip": fill(t("filesRemoveFolder"), { folder }) }, remove));
      return row;
    });
    // Rows change only when the folders do, so the list does not rebuild on each clock tick.
    const signature = JSON.stringify([folders, host.language()]);
    if (list.dataset.signature !== signature) {
      list.dataset.signature = signature;
      list.replaceChildren(...rows);
      host.refreshIcons();
    }
  }

  function render(): void {
    if (!device) return;
    scopeChoice.select(scope);
    modeChoice.select(mode);
    durationChoice.select(duration);
    foldersBox.set(scope === "folders");
    wideBox.set(scope !== "folders");
    renderFolders();
    const out = folders.length === 0 && scope === "folders";
    // The last text stays while the note closes.
    if (out) setText(foldersNote, t("filesFoldersNone"));
    foldersNoteBox.set(out);
    add.disabled = folders.length >= MAX_FOLDERS || picking;
    const writing = mode === "readWrite";
    const warningText = t(scope === "home" ? (writing ? "filesHomeWarning" : "filesHomeWarningRead")
      : (writing ? "filesDiskWarning" : "filesDiskWarningRead"));
    // The first sentence is bold, and the rest follows it.
    const stop = warningText.search(/[.。]\s/);
    const lead = stop < 0 ? warningText : warningText.slice(0, stop + 1);
    const rest = stop < 0 ? "" : ` ${warningText.slice(stop + 1).trim()}`;
    fadeText(warningLead, lead);
    fadeText(warningRest, rest);
    const home = host.home();
    if (home !== null) setText(homePath, home);
    homeBox.set(scope === "home" && home !== null);
    allowsBox.set(scope === "folders");
    fadeText(allows, fill(t(mode === "readWrite" ? "filesNoteFoldersWrite" : "filesNoteFoldersRead"), { name: device.name }));
    const custom = duration === "custom";
    for (const field of [hoursField, minutesField]) if (field.disabled !== !custom) field.disabled = !custom;
    const minutes = chosen();
    if (minutes === null) setText(endNote, t("shellEndsNever"));
    else if (minutes === undefined) setText(endNote, t("shellTimeInvalid"));
    else {
      const end = Math.floor(Date.now() / 1000) + minutes * 60;
      setText(endNote, fill(t(isToday(end) ? "shellEndsToday" : "shellEndsLater"), { time: formatClock(end, host.language()) }));
    }
    endNote.toggleAttribute("data-warning", minutes === undefined);
    const ready = minutes !== undefined && !saving && !out;
    if (allow.disabled !== !ready) allow.disabled = !ready;
    setText(title, step === "form" ? fill(t("filesGrantTitle"), { name: device.name }) : t("filesPrivacyTitle"));
    renderPrivacy();
  }

  /** The step after saving: the questions of macOS while they are asked, then what each place answered. */
  function renderPrivacy(): void {
    formStep.hidden = step !== "form";
    privacyStep.hidden = step === "form";
    progressBox.set(step === "wait");
    const wide = saved !== "folders";
    fdaBox.set(wide && step !== "form");
    const closed = checks.filter(check => check.touch !== "readable").length;
    // The last text stays while the line closes.
    if (step === "result") fadeText(privacySummary, closed === 0 ? t("filesPrivacyAllOk") : fill(t("filesPrivacyClosed"), { count: closed, total: checks.length }));
    privacySummary.toggleAttribute("data-warning", step === "result" && closed > 0);
    const signature = JSON.stringify([checks, host.language()]);
    if (privacyList.dataset.signature !== signature) {
      privacyList.dataset.signature = signature;
      privacyList.replaceChildren(...checks.map(check => {
        const look = touches[check.touch];
        const known = places[check.path];
        return h("li", { class: "files-privacy-row", "data-touch": check.touch },
          icon(look.icon),
          h("span", { class: "files-privacy-place" }, known ? t(known) : check.path),
          h("span", { class: "files-privacy-path" }, known ? check.path : ""),
          h("span", { class: "files-privacy-state" }, t(look.label)));
      }));
      host.refreshIcons();
    }
  }

  function open(next: { id: string; name: string }, from: HTMLElement): void {
    device = next;
    trigger = from;
    saving = false;
    session += 1;
    step = "form";
    checks = [];
    scope = "folders";
    mode = "read";
    duration = "hour";
    folders = [SUGGESTED];
    fields(presets.hour);
    delete list.dataset.signature;
    delete privacyList.dataset.signature;
    render();
    // The window always opens at its top, whatever the last one showed.
    body.scrollTop = 0;
    shell.setAttribute("data-open", "");
    shell.inert = false;
    window.clearInterval(ticker);
    ticker = window.setInterval(render, clockTick);
    host.refreshIcons();
    allow.focus({ preventScroll: true });
  }

  function closeWindow(restoreFocus = true): void {
    if (!shell.hasAttribute("data-open")) return;
    shell.removeAttribute("data-open");
    shell.inert = true;
    window.clearInterval(ticker);
    ticker = undefined;
    device = undefined;
    saving = false;
    session += 1;
    if (restoreFocus && trigger?.isConnected) trigger.focus();
  }

  add.addEventListener("click", () => {
    if (picking) return;
    picking = true;
    render();
    void desktop.pickFolder().then(folder => {
      if (folder !== null && !folders.includes(folder) && folders.length < MAX_FOLDERS) folders = [...folders, folder];
    }, (error: unknown) => host.fail(error)).finally(() => {
      picking = false;
      render();
    });
  });
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
    if (!device || saving || minutes === undefined || step !== "form") return;
    saving = true;
    render();
    const mine = ++session;
    const chosenScope = scope;
    // Saving the grant also reads the protected places, which holds the answer while macOS asks its questions.
    const slow = window.setTimeout(() => {
      if (mine !== session) return;
      saved = chosenScope;
      step = "wait";
      render();
      body.scrollTop = 0;
    }, PROGRESS_DELAY);
    void desktop.grantFiles(device.id, chosenScope, chosenScope === "folders" ? folders : [], mode, minutes).then(granted => {
      window.clearTimeout(slow);
      host.changed();
      if (mine !== session) return;
      saving = false;
      if (granted.privacy.length === 0) {
        closeWindow(false);
        return;
      }
      saved = chosenScope;
      checks = granted.privacy;
      step = "result";
      render();
      body.scrollTop = 0;
      done.focus();
    }, (error: unknown) => {
      window.clearTimeout(slow);
      if (mine !== session) return;
      saving = false;
      step = "form";
      render();
      host.fail(error);
    });
  });
  done.addEventListener("click", () => closeWindow());
  fdaButton.addEventListener("click", () => { void desktop.openFullDiskAccess().catch((error: unknown) => host.fail(error)); });

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
