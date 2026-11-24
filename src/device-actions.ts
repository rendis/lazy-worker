import { desktop, type ActionRun, type ActionSchedule, type SavedAction } from "./desktop";
import type { Language, Message } from "./messages";
import { h, icon } from "./proxy";
import { fadeText, only, setHidden } from "./motion";
import { isLinkError } from "./remote";
import { formatDuration, formatLimit, DURATION_WIDTH } from "./time";

// The windows of the actions of a device: the form of an action, its schedule, and the result of a run.
// Names, commands, and output can come from another device, so they are only ever set as text.

/** The device that an action belongs to. `id` is `null` for this device. */
export type Target = { id: string | null; name: string };

type Host = {
  translate(key: Message): string;
  language(): Language;
  refreshIcons(): void;
  /** Shows the notice of a failed change. `error` is the code that the device sent, when it sent one. */
  fail(error: unknown): void;
  /** Called when something changed on a device, so the lists read it again. */
  changed(): void;
};

/** The limits that the form offers. An action that has another limit keeps it as one more choice. */
const limits = [30, 60, 300, 900, 1800, 3600];
const defaultLimit = 300;
const maxCommandBytes = 4096;
const runPoll = 1000;
const copiedFor = 1400;
const defaultTime = "02:00";
const stateMessages: Record<ActionRun["state"], Message> = {
  running: "runRunning", finished: "runFinished", stopped: "runStopped", timedOut: "runTimedOut", failed: "runNotStarted",
};

function setText(node: HTMLElement, text: string): void {
  if (node.textContent !== text) node.textContent = text;
}

function fill(text: string, values: Record<string, string | number>): string {
  return text.replace(/\{(\w+)\}/g, (marker, key: string) => key in values ? String(values[key]) : marker);
}

/** `YYYY-MM-DD` and `HH:MM` on the clock of this window. */
function localParts(epochSeconds: number): { date: string; time: string } {
  const moment = new Date(epochSeconds * 1000);
  const pad = (value: number): string => String(value).padStart(2, "0");
  return {
    date: `${moment.getFullYear()}-${pad(moment.getMonth() + 1)}-${pad(moment.getDate())}`,
    time: `${pad(moment.getHours())}:${pad(moment.getMinutes())}`,
  };
}

export function mountActionWindows(host: Host) {
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

  /** A window that opens over the view and gives the focus back to what opened it. */
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

  /** A note that opens and closes over a short time. */
  function note(warning = false) {
    const text = h("span");
    const root = h("div", { class: "collapse", inert: true },
      h("div", {}, h("p", { class: "setting-note action-note", "data-warning": warning }, text)));
    return {
      root,
      set(message: string): void {
        // The last text stays while the note closes.
        if (message !== "") fadeText(text, message);
        root.toggleAttribute("data-open", message !== "");
        root.toggleAttribute("inert", message === "");
      },
    };
  }

  function row(name: string, label: Message, ...end: (HTMLElement | undefined)[]) {
    return h("div", { class: "setting-row" }, icon(name), bind(h("span", { class: "setting-label" }), label),
      h("div", { class: "device-end" }, ...end));
  }

  // ---- The form: the name, the command, and the time limit of an action of this device ----
  const formWindow = document.getElementById("action-form-window")!;
  const formTitle = document.getElementById("action-form-title")!;
  const formBody = document.getElementById("action-form-body") as HTMLFormElement;
  const formClose = document.getElementById("action-form-close") as HTMLButtonElement;
  let editing: SavedAction | undefined;
  let saving = false;

  const nameField = h("input", {
    class: "field action-name", type: "text", maxlength: "60", autocomplete: "off", spellcheck: "false", required: true,
  });
  bind(nameField, "actionName", "aria-label");
  const commandField = h("textarea", {
    class: "field action-command", rows: "5", autocomplete: "off", autocapitalize: "off", spellcheck: "false", required: true,
  });
  bind(commandField, "actionCommand", "aria-label");
  const limitField = h("select", { class: "field action-limit" });
  bind(limitField, "actionTimeLimit", "aria-label");
  const warning = h("p", { class: "setting-note action-warning", "data-warning": true },
    icon("triangle-alert"), bind(h("span"), "actionWarning"));
  const formError = note(true);
  const formCancel = h("button", { type: "button", class: "text-button" });
  bind(formCancel, "cancel");
  const formSave = h("button", { type: "submit", class: "access-action" }, icon("check"), bind(h("span"), "save"));
  // Only the person at this device tries a command, and only on this device: another device's window never shows these.
  function tipButton(name: string, key: Message) {
    const button = h("button", { type: "button", class: "icon-button outlined" }, icon(name));
    const root = h("span", { class: "tip", "data-tip": t(key) }, button);
    const apply = (): void => {
      root.setAttribute("data-tip", t(key));
      button.setAttribute("aria-label", t(key));
    };
    apply();
    labelled.push(apply);
    return { root, button };
  }
  const testButton = tipButton("terminal", "actionTestInTerminal");
  const copyCommandButton = tipButton("copy", "actionCopyCommand");
  const commandCopied = h("span", { class: "copy-confirm", role: "status" });
  let commandCopiedTimer: number | undefined;
  commandField.id = "action-command-field";
  const commandTools = desktop.remote === null
    ? h("div", { class: "device-end shell-buttons" }, commandCopied, testButton.root, copyCommandButton.root)
    : undefined;
  const commandLabel = h("div", { class: "action-field" },
    h("div", { class: "action-field-head" },
      h("label", { class: "setting-label", for: "action-command-field" }, icon("square-terminal"), bind(h("span"), "actionCommand")),
      commandTools),
    commandField);
  formBody.append(
    row("tag", "actionName", nameField),
    commandLabel,
    row("timer", "actionTimeLimit", limitField),
    warning, formError.root,
    h("div", { class: "action-footer" }, formCancel, formSave));

  function fillLimits(current: number): void {
    const values = limits.includes(current) ? limits : [...limits, current].sort((a, b) => a - b);
    limitField.replaceChildren(...values.map(value => {
      const option = h("option", { value: String(value) }, formatLimit(value));
      return option;
    }));
    limitField.value = String(current);
  }

  const formOverlay = overlay(formWindow, formClose, () => {
    editing = undefined;
    saving = false;
    nameField.value = "";
    commandField.value = "";
    formError.set("");
  });

  function openForm(action: SavedAction | undefined, trigger: HTMLElement): void {
    editing = action;
    setText(formTitle, t(action ? "actionFormEdit" : "actionFormNew"));
    nameField.value = action?.name ?? "";
    commandField.value = action?.command ?? "";
    fillLimits(action?.timeoutSeconds ?? defaultLimit);
    formError.set("");
    formSave.disabled = false;
    formOverlay.open(trigger, nameField);
  }

  function formProblem(): Message | undefined {
    if (nameField.value.trim() === "") return "actionNameRequired";
    if (commandField.value.trim() === "") return "actionCommandRequired";
    if (new TextEncoder().encode(commandField.value).length > maxCommandBytes) return "actionCommandTooLong";
    return undefined;
  }

  formBody.addEventListener("submit", event => {
    event.preventDefault();
    if (saving) return;
    const problem = formProblem();
    formError.set(problem ? t(problem) : "");
    if (problem) return;
    saving = true;
    formSave.disabled = true;
    const draft = {
      name: nameField.value.trim(), command: commandField.value.replace(/\0/g, ""), timeoutSeconds: Number(limitField.value),
    };
    const saved = editing ? desktop.updateAction(editing.id, draft) : desktop.createAction(draft);
    void saved.then(() => {
      saving = false;
      formOverlay.close(false);
      host.changed();
    }, (error: unknown) => {
      saving = false;
      formSave.disabled = false;
      host.fail(error);
    });
  });
  formCancel.addEventListener("click", () => formOverlay.close());
  nameField.addEventListener("input", () => { if (formProblem() === undefined) formError.set(""); });
  commandField.addEventListener("input", () => { if (formProblem() === undefined) formError.set(""); });

  /** The command as typed, or nothing with the reason in the form's note. */
  function commandToTry(): string | undefined {
    const problem = commandField.value.trim() === "" ? "actionCommandRequired"
      : new TextEncoder().encode(commandField.value).length > maxCommandBytes ? "actionCommandTooLong" : undefined;
    if (problem) formError.set(t(problem));
    return problem ? undefined : commandField.value.replace(/\0/g, "");
  }
  testButton.button.addEventListener("click", () => {
    const command = commandToTry();
    if (command !== undefined) void desktop.testActionCommand(command).catch((error: unknown) => host.fail(error));
  });
  copyCommandButton.button.addEventListener("click", () => {
    const command = commandToTry();
    if (command === undefined) return;
    void desktop.copyActionCommandLine(command).then(() => {
      setText(commandCopied, t("actionCommandCopied"));
      commandCopied.setAttribute("data-show", "");
      window.clearTimeout(commandCopiedTimer);
      commandCopiedTimer = window.setTimeout(() => commandCopied.removeAttribute("data-show"), copiedFor);
    }).catch((error: unknown) => host.fail(error));
  });

  // ---- The schedule of an action, on this device or on another one ----
  const scheduleWindow = document.getElementById("action-schedule-window")!;
  const scheduleTitle = document.getElementById("action-schedule-title")!;
  const scheduleSubtitle = document.getElementById("action-schedule-subtitle")!;
  const scheduleBody = document.getElementById("action-schedule-body") as HTMLFormElement;
  const scheduleClose = document.getElementById("action-schedule-close") as HTMLButtonElement;
  type Repeat = "once" | "daily" | "weekly";
  const repeats: { id: Repeat; label: Message }[] = [
    { id: "once", label: "repeatOnce" }, { id: "daily", label: "repeatDaily" }, { id: "weekly", label: "repeatWeekly" },
  ];
  let scheduling: { target: Target; action: SavedAction } | undefined;
  let repeat: Repeat = "daily";
  let removing = false;
  let scheduleSaving = false;

  const repeatButtons = repeats.map(({ id, label }) => {
    const button = h("button", { type: "button", "aria-pressed": "false" });
    bind(button, label);
    button.addEventListener("click", () => {
      repeat = id;
      scheduleError.set("");
      renderSchedule();
    });
    return { id, button };
  });
  const repeatGroup = h("div", { class: "segments action-repeat", role: "group", style: "--segment-count: 3" },
    ...repeatButtons.map(({ button }) => button), h("span", { class: "segment-indicator", "aria-hidden": "true" }));
  const repeatRow = h("div", { class: "setting-row action-repeat-row" },
    icon("repeat"), bind(h("span", { class: "setting-label" }), "scheduleRepeat"), h("div", { class: "device-end" }, repeatGroup));
  repeatGroup.setAttribute("aria-label", t("scheduleRepeat"));
  labelled.push(() => repeatGroup.setAttribute("aria-label", t("scheduleRepeat")));

  const dateField = h("input", { class: "field", type: "date", required: true });
  bind(dateField, "scheduleDate", "aria-label");
  const dayField = h("select", { class: "field" });
  bind(dayField, "scheduleDay", "aria-label");
  const timeField = h("input", { class: "field", type: "time", required: true, step: "60" });
  bind(timeField, "scheduleTime", "aria-label");
  const dateRow = row("calendar", "scheduleDate", dateField);
  const dayRow = row("calendar-days", "scheduleDay", dayField);
  const timeRow = row("clock", "scheduleTime", timeField);
  const clockNote = h("p", { class: "setting-note action-clock-note" });
  const flagInput = h("input", { type: "checkbox", role: "switch" });
  flagInput.setAttribute("switch", "");
  const flagText = h("span", { class: "action-flag-text" },
    bind(h("span", { class: "setting-label" }), "scheduleFlag"), bind(h("span", { class: "setting-note" }), "scheduleFlagHint"));
  const flagRow = h("label", { class: "setting-row action-flag" }, icon("bell"), flagText,
    h("div", { class: "device-end" }, h("span", { class: "switch" }, flagInput, h("span", { "aria-hidden": "true" }))));
  const logNote = bind(h("p", { class: "setting-note action-log-note" }), "scheduleLogNote");
  const scheduleError = note(true);

  const removeButton = h("button", { type: "button", class: "text-button danger" });
  bind(removeButton, "scheduleRemove");
  const removeAsk = h("span", { class: "action-remove-ask", hidden: true },
    bind(h("span", { class: "setting-note" }), "scheduleRemoveConfirm"),
    bind(h("button", { type: "button", class: "text-button danger", "data-remove": "yes" }), "scheduleRemoveAccept"),
    bind(h("button", { type: "button", class: "text-button", "data-remove": "no" }), "cancel"));
  const scheduleCancel = bind(h("button", { type: "button", class: "text-button" }), "cancel");
  const scheduleSave = h("button", { type: "submit", class: "access-action" }, icon("check"), bind(h("span"), "save"));
  scheduleBody.append(
    repeatRow, dateRow, dayRow, timeRow, clockNote, flagRow, logNote, scheduleError.root,
    h("div", { class: "action-footer" }, removeButton, removeAsk, h("span", { class: "action-spacer" }), scheduleCancel, scheduleSave));

  function weekday(day: number): string {
    // 7 January 2024 is a Sunday.
    return new Date(2024, 0, 7 + day).toLocaleDateString(host.language(), { weekday: "long" });
  }

  function fillDays(): void {
    const keep = dayField.value;
    dayField.replaceChildren(...Array.from({ length: 7 }, (_, day) => h("option", { value: String(day) }, weekday(day))));
    dayField.value = keep || "1";
  }

  function renderSchedule(): void {
    if (!scheduling) return;
    for (const { id, button } of repeatButtons) button.setAttribute("aria-pressed", String(id === repeat));
    setHidden(dateRow, repeat !== "once");
    setHidden(dayRow, repeat !== "weekly");
    setHidden(clockNote, repeat === "once");
    const { target } = scheduling;
    setText(clockNote, fill(t("scheduleClockNote"), { device: target.id === null ? t("scheduleClockHere") : target.name }));
    setText(scheduleTitle, fill(t("scheduleTitle"), { action: scheduling.action.name }));
    setText(scheduleSubtitle, target.id === null ? t("scheduleRunsHere") : fill(t("scheduleRunsOn"), { device: target.name }));
    only([removeButton, removeAsk], removing ? [removeAsk] : scheduling.action.schedule ? [removeButton] : []);
    const ready = repeat === "once" ? dateField.value !== "" && timeField.value !== "" : timeField.value !== "";
    scheduleSave.disabled = !ready || scheduleSaving;
  }

  labelled.push(() => {
    fillDays();
    renderSchedule();
  });

  const scheduleOverlay = overlay(scheduleWindow, scheduleClose, () => {
    scheduling = undefined;
    removing = false;
    scheduleSaving = false;
    scheduleError.set("");
  });

  function openSchedule(target: Target, action: SavedAction, trigger: HTMLElement): void {
    scheduling = { target, action };
    removing = false;
    scheduleSaving = false;
    scheduleError.set("");
    const current = action.schedule;
    repeat = current?.kind ?? "daily";
    timeField.value = defaultTime;
    dayField.value = "1";
    dateField.value = "";
    if (current?.kind === "once") {
      const parts = localParts(current.at);
      dateField.value = parts.date;
      timeField.value = parts.time;
    } else if (current) {
      timeField.value = current.time;
      if (current.kind === "weekly") dayField.value = String(current.day);
    }
    if (repeat === "once" && dateField.value === "") dateField.value = localParts(Date.now() / 1000 + 3600).date;
    fillDays();
    if (current?.kind === "weekly") dayField.value = String(current.day);
    flagInput.checked = action.notifyOnFailure;
    renderSchedule();
    scheduleOverlay.open(trigger, scheduleSave);
  }

  /** The schedule that the fields say, or `undefined` for a moment that cannot be. */
  function chosenSchedule(): ActionSchedule | undefined {
    if (repeat === "daily") return { kind: "daily", time: timeField.value };
    if (repeat === "weekly") return { kind: "weekly", day: Number(dayField.value), time: timeField.value };
    const [year, month, day] = dateField.value.split("-").map(Number);
    const [hours, minutes] = timeField.value.split(":").map(Number);
    const at = Math.floor(new Date(year, month - 1, day, hours, minutes).getTime() / 1000);
    const now = Date.now() / 1000;
    return at > now && at <= now + 366 * 86400 ? { kind: "once", at } : undefined;
  }

  function finishSchedule(work: Promise<void>): void {
    scheduleSaving = true;
    renderSchedule();
    void work.then(() => {
      scheduleSaving = false;
      scheduleOverlay.close(false);
      host.changed();
    }, (error: unknown) => {
      scheduleSaving = false;
      renderSchedule();
      if (error === "scheduleInvalid") scheduleError.set(t("scheduleInvalid"));
      else host.fail(error);
    });
  }

  scheduleBody.addEventListener("submit", event => {
    event.preventDefault();
    if (!scheduling || scheduleSaving) return;
    const schedule = chosenSchedule();
    if (!schedule) {
      scheduleError.set(t("scheduleOnceInvalid"));
      return;
    }
    scheduleError.set("");
    finishSchedule(desktop.scheduleAction(scheduling.target.id, scheduling.action.id, schedule, flagInput.checked));
  });
  for (const field of [dateField, dayField, timeField]) {
    field.addEventListener("input", () => {
      scheduleError.set("");
      renderSchedule();
    });
  }
  removeButton.addEventListener("click", () => {
    removing = true;
    renderSchedule();
    removeAsk.querySelector<HTMLButtonElement>("[data-remove=no]")!.focus();
  });
  removeAsk.addEventListener("click", event => {
    const choice = (event.target as Element).closest<HTMLElement>("[data-remove]")?.dataset.remove;
    if (!choice || !scheduling) return;
    if (choice === "no") {
      removing = false;
      renderSchedule();
      removeButton.focus();
    } else {
      removing = false;
      finishSchedule(desktop.clearSchedule(scheduling.target.id, scheduling.action.id));
    }
  });
  scheduleCancel.addEventListener("click", () => scheduleOverlay.close());

  // ---- The result of a run: its state, what it printed, and Stop while it goes ----
  const runWindow = document.getElementById("action-run-window")!;
  const runTitle = document.getElementById("action-run-title")!;
  const runSubtitle = document.getElementById("action-run-subtitle")!;
  const runBody = document.getElementById("action-run-body")!;
  const runClose = document.getElementById("action-run-close") as HTMLButtonElement;
  type Run = { target: Target; runId: string; last?: ActionRun; error?: string; stopping: boolean; timer?: number; busy: boolean };
  let run: Run | undefined;

  const statusIcons = {
    running: h("span", { class: "run-icon", "data-kind": "running" }, icon("loader-circle")),
    ok: h("span", { class: "run-icon", "data-kind": "ok" }, icon("circle-check")),
    bad: h("span", { class: "run-icon", "data-kind": "bad" }, icon("circle-x")),
    stopped: h("span", { class: "run-icon", "data-kind": "stopped" }, icon("circle-stop")),
  };
  const statusText = h("span", { class: "run-status-text" });
  const durationText = h("span", { class: "run-duration" });
  durationText.style.minInlineSize = `${DURATION_WIDTH}ch`;
  const statusLine = h("p", { class: "run-status", role: "status" }, ...Object.values(statusIcons), statusText, durationText);
  const output = h("pre", { class: "run-output", tabindex: "0" });
  const outputEmpty = h("p", { class: "setting-note run-empty" });
  const outputBox = h("div", { class: "run-output-box" }, output, outputEmpty);
  const runNote = note(false);
  const stopButton = h("button", { type: "button", class: "text-button" }, icon("square"), bind(h("span"), "runStop"));
  const copyLabel = bind(h("span"), "runCopyOutput");
  const copyButton = h("button", { type: "button", class: "text-button" }, icon("copy"), copyLabel);
  const closeButton = bind(h("button", { type: "button", class: "text-button" }), "close");
  runBody.append(statusLine, outputBox, runNote.root,
    h("div", { class: "action-footer" }, stopButton, h("span", { class: "action-spacer" }), copyButton, closeButton));
  let copiedTimer: number | undefined;

  function stopPolling(): void {
    if (run?.timer !== undefined) window.clearInterval(run.timer);
    if (run) run.timer = undefined;
  }

  const runOverlay = overlay(runWindow, runClose, () => {
    stopPolling();
    run = undefined;
    window.clearTimeout(copiedTimer);
    output.textContent = "";
  });

  function renderRun(): void {
    if (!run) return;
    const { last } = run;
    const state = last?.state;
    const failed = state === "failed" || state === "timedOut" || (state === "finished" && last!.exitCode !== 0);
    const kind = !last || state === "running" ? "running" : state === "stopped" ? "stopped" : failed ? "bad" : "ok";
    only(Object.values(statusIcons), [statusIcons[kind]]);
    statusLine.dataset.kind = kind;
    const parts: string[] = [];
    if (!last) parts.push(t("runStarting"));
    else if (run.stopping && state === "running") parts.push(t("runStopping"));
    else parts.push(t(failed && state === "finished" ? "runFailed" : stateMessages[last.state]));
    if (last && state !== "running" && last.exitCode !== null) parts.push(fill(t("runCode"), { code: last.exitCode }));
    fadeText(statusText, parts.join(" · "));
    setText(durationText, last && state !== "failed" ? formatDuration(last.seconds) : "");
    const text = last?.output ?? "";
    if (output.textContent !== text) {
      const follow = output.scrollHeight - output.scrollTop - output.clientHeight < 24;
      output.textContent = text;
      if (follow) output.scrollTop = output.scrollHeight;
    }
    only([output, outputEmpty], [text === "" ? outputEmpty : output]);
    fadeText(outputEmpty, t(state === "running" || !last ? "runWaitingOutput" : "runNoOutput"));
    const message = run.error ? t(isLinkError(run.error) ? "remoteLost" : run.error === "actionRunNotFound" ? "runGone" : "actionsUnavailable")
      : last?.truncated ? t("runTruncated") : "";
    runNote.set(message);
    setHidden(stopButton, state !== "running");
    stopButton.disabled = run.stopping || run.busy;
    copyButton.disabled = text === "";
  }

  async function readRun(current: Run): Promise<void> {
    if (current.busy) return;
    current.busy = true;
    try {
      const view = await desktop.getActionRun(current.target.id, current.runId);
      if (run !== current) return;
      current.last = view;
      current.error = undefined;
      if (view.state !== "running") {
        stopPolling();
        host.changed();
      }
    } catch (error) {
      if (run !== current) return;
      current.error = typeof error === "string" ? error : "remoteFailed";
      // A run that the device does not know any more will not come back. A lost link may.
      if (!isLinkError(error)) stopPolling();
    } finally {
      current.busy = false;
    }
    renderRun();
  }

  /** Opens the window for a run that the device just accepted, and reads it every second until it ends. */
  function openRun(target: Target, action: { name: string }, runId: string, trigger: HTMLElement | undefined): void {
    runOverlay.close(false);
    const current: Run = { target, runId, stopping: false, busy: false };
    run = current;
    setText(runTitle, action.name);
    setText(runSubtitle, target.id === null ? t("thisDevice") : target.name);
    renderRun();
    // The stop button shows after the first answer, so the focus starts on Close.
    runOverlay.open(trigger, closeButton);
    current.timer = window.setInterval(() => void readRun(current), runPoll);
    void readRun(current);
  }

  stopButton.addEventListener("click", () => {
    const current = run;
    if (!current || current.stopping) return;
    current.stopping = true;
    renderRun();
    void desktop.stopActionRun(current.target.id, current.runId).then(() => readRun(current), (error: unknown) => {
      current.stopping = false;
      host.fail(error);
      renderRun();
    });
  });
  copyButton.addEventListener("click", () => {
    const text = run?.last?.output ?? "";
    if (text === "") return;
    void desktop.copyText(text).then(() => {
      fadeText(copyLabel, t("runCopied"));
      window.clearTimeout(copiedTimer);
      copiedTimer = window.setTimeout(() => fadeText(copyLabel, t("runCopyOutput")), copiedFor);
    }).catch(() => {});
  });
  closeButton.addEventListener("click", () => runOverlay.close());

  return {
    openForm,
    openSchedule,
    openRun,
    /** Closes the topmost window. It returns true when one was open. */
    closeTop(): boolean {
      for (const shown of [formOverlay, scheduleOverlay, runOverlay]) {
        if (!shown.isOpen()) continue;
        shown.close();
        return true;
      }
      return false;
    },
    closeAll(): void {
      formOverlay.close(false);
      scheduleOverlay.close(false);
      runOverlay.close(false);
    },
    /** Puts the words of the windows in the language of the interface. */
    retranslate(): void {
      for (const apply of labelled) apply();
      if (formOverlay.isOpen()) setText(formTitle, t(editing ? "actionFormEdit" : "actionFormNew"));
      renderSchedule();
      renderRun();
    },
  };
}
