import { desktop, type MailboxCheck, type MailboxConversation, type MailboxItem, type MailboxParty } from "./desktop";
import { formatBytes } from "./files";
import { messages, type Language, type Message } from "./messages";
import { age, type AgeUnit } from "./time";

const ageMessages: Record<Exclude<AgeUnit, "seconds">, Message> = {
  minutes: "minutesAgo", hours: "hoursAgo", days: "daysAgo",
};
/** How often an open conversation is read while something moves, and while nothing does, in milliseconds. */
const busyPoll = 700;
const idlePoll = 5000;
/** How often an open window asks its device for what waits for this one, and reads whether the device answers. */
const devicePoll = 10000;
/** How long the confirmation of a copied text shows, in milliseconds. */
const copiedFor = 1400;

function fill(text: string, values: Record<string, string | number>): string {
  return text.replace(/\{(\w+)\}/g, (match, name: string) => (name in values ? String(values[name]) : match));
}

function isMessage(key: string): key is Message {
  return Object.prototype.hasOwnProperty.call(messages.en, key);
}

function sameParty(a: MailboxParty, b: MailboxParty): boolean {
  return "device" in a ? "device" in b && a.device === b.device : "peer" in b && a.peer === b.peer;
}

function moving(item: MailboxItem): boolean {
  return ["compressing", "queued", "sending", "arriving", "pickingUp"].includes(item.status);
}

/** The error words of a check that did not reach the device, in plain words. Any other word is a connection problem. */
const reasonMessages: Record<string, Message> = {
  sshUnreachable: "mailboxReasonNetwork", remoteUnavailable: "mailboxReasonNetwork", sshFailed: "mailboxReasonNetwork",
  sshTimeout: "mailboxReasonTimeout", remoteTimeout: "mailboxReasonTimeout",
  sshDenied: "mailboxReasonKey", sshHostKeyChanged: "mailboxReasonKey", sshHostKeyUnknown: "mailboxReasonKey", notPaired: "mailboxReasonKey",
  appNotRunning: "mailboxReasonNotRunning", busy: "mailboxReasonBusy", badRequest: "mailboxReasonVersion",
};

/**
 * Why the mailbox with a device that this one controls cannot be used now, in plain words, with when that was checked.
 * Nothing while the device allows it. The device row shows the same text as the tip of its mailbox button.
 */
export function mailboxBlocked(
  granted: boolean | null, check: MailboxCheck | null, name: string, translate: (message: Message) => string, language: Language,
): { text: string; checked: string } | undefined {
  const checked = check ? fill(translate("mailboxChecked"), { age: ageText(check.at, translate, language) }) : "";
  if (check?.outcome === "unreachable") {
    const reason = translate(reasonMessages[check.reason ?? ""] ?? "mailboxReasonOther");
    return { text: fill(translate("mailboxUnreachableNote"), { name, reason }), checked };
  }
  if (granted === true) return undefined;
  if (granted === false) return { text: fill(translate("mailboxOffNote"), { name }), checked };
  return { text: fill(translate("mailboxNotCheckedNote"), { name }), checked };
}

function ageText(at: number, translate: (message: Message) => string, _language: Language): string {
  const { value, unit } = age(Date.now() / 1000 - at);
  return unit === "seconds" ? translate("justNow") : fill(translate(ageMessages[unit]), { n: value });
}

type Action = { root: HTMLElement; button: HTMLButtonElement };
type Row = {
  root: HTMLLIElement;
  bubble: HTMLElement;
  text: HTMLElement;
  file: HTMLElement;
  fileIcon: HTMLElement;
  fileName: HTMLElement;
  fileSize: HTMLElement;
  bar: HTMLElement;
  fill: HTMLElement;
  status: HTMLElement;
  actions: Record<"copy" | "open" | "reveal" | "retry" | "cancel" | "remove", Action>;
  item: MailboxItem;
  iconName: string;
};

/** The mailbox window with one device. Every text and name that a device sent is set as text only. */
export function mountMailbox({ translate, language, refreshIcons, onChange }: {
  translate: (message: Message) => string;
  language: () => Language;
  refreshIcons: () => void;
  /** A conversation changed, so badges can read the summary again. */
  onChange: () => void;
}) {
  const overlay = document.getElementById("mailbox-window")!;
  const title = document.getElementById("mailbox-title")!;
  const state = document.getElementById("mailbox-state")!;
  const close = document.getElementById("mailbox-close") as HTMLButtonElement;
  const body = document.getElementById("mailbox-body")!;
  const note = document.getElementById("mailbox-note")!;
  const noteText = document.getElementById("mailbox-note-text")!;
  const noteChecked = document.getElementById("mailbox-checked")!;
  const checkNow = document.getElementById("mailbox-check-now") as HTMLButtonElement;
  const list = document.getElementById("mailbox-list")!;
  const empty = document.getElementById("mailbox-empty")!;
  const drop = document.getElementById("mailbox-drop")!;
  const dropText = document.getElementById("mailbox-drop-text")!;
  const input = document.getElementById("mailbox-input") as HTMLTextAreaElement;
  const filesButton = document.getElementById("mailbox-files") as HTMLButtonElement;
  const folderButton = document.getElementById("mailbox-folder") as HTMLButtonElement;
  const send = document.getElementById("mailbox-send") as HTMLButtonElement;
  const error = document.getElementById("mailbox-error")!;
  const idle = document.getElementById("mailbox-footer-idle")!;
  const confirm = document.getElementById("mailbox-footer-confirm")!;
  const folderText = document.getElementById("mailbox-folder-text")!;
  const changeFolder = document.getElementById("mailbox-change-folder") as HTMLButtonElement;
  const clear = document.getElementById("mailbox-clear") as HTMLButtonElement;
  const clearCancel = document.getElementById("mailbox-clear-cancel") as HTMLButtonElement;
  const clearYes = document.getElementById("mailbox-clear-yes") as HTMLButtonElement;
  const rows = new Map<string, Row>();
  let party: MailboxParty | undefined;
  let name = "";
  let online: boolean | null = null;
  let conversation: MailboxConversation | undefined;
  let revision = 0;
  let timer: number | undefined;
  let liveTimer: number | undefined;
  let trigger: HTMLElement | undefined;
  let renderedLanguage: Language | undefined;

  const locale = () => (language() === "es" ? "es" : "en");
  const size = (bytes: number) => formatBytes(bytes, locale());

  function elapsed(at: number): string {
    const { value, unit } = age(Date.now() / 1000 - at);
    return unit === "seconds" ? translate("justNow") : fill(translate(ageMessages[unit]), { n: value });
  }

  function action(label: Message, iconName: string, run: (row: Row) => void, row: () => Row): Action {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "icon-button mailbox-action";
    button.setAttribute("aria-label", translate(label));
    button.dataset.label = label;
    const glyph = document.createElement("i");
    glyph.dataset.lucide = iconName;
    button.append(glyph);
    const root = document.createElement("span");
    root.className = "tip";
    root.dataset.tip = translate(label);
    root.dataset.tipLabel = label;
    root.append(button);
    button.addEventListener("click", () => run(row()));
    return { root, button };
  }

  function showError(code: unknown): void {
    const key = typeof code === "string" && isMessage(code) ? code : "remoteChangeFailed";
    error.textContent = translate(key);
    error.hidden = false;
  }

  function run(work: Promise<void>): void {
    error.hidden = true;
    work.then(() => refresh()).catch(code => showError(code));
  }

  function build(item: MailboxItem): Row {
    const root = document.createElement("li");
    const bubble = document.createElement("div");
    bubble.className = "mailbox-bubble";
    const text = document.createElement("p");
    text.className = "mailbox-text";
    const file = document.createElement("div");
    file.className = "mailbox-file";
    const fileIcon = document.createElement("span");
    fileIcon.className = "mailbox-file-icon";
    const fileText = document.createElement("span");
    fileText.className = "mailbox-file-text";
    const fileName = document.createElement("strong");
    const fileSize = document.createElement("span");
    fileText.append(fileName, fileSize);
    file.append(fileIcon, fileText);
    const bar = document.createElement("span");
    bar.className = "mailbox-bar";
    bar.setAttribute("aria-hidden", "true");
    const fillNode = document.createElement("span");
    bar.append(fillNode);
    const status = document.createElement("span");
    status.className = "mailbox-status";
    const actionsBox = document.createElement("span");
    actionsBox.className = "mailbox-actions";
    let self: Row;
    const current = () => self;
    const target = () => party!;
    const actions = {
      copy: action("mailboxCopy", "copy", row => {
        void desktop.copyText(row.item.text ?? "").then(() => {
          row.actions.copy.root.dataset.tip = translate("mailboxCopied");
          window.setTimeout(() => { row.actions.copy.root.dataset.tip = translate("mailboxCopy"); }, copiedFor);
        }).catch(code => showError(code));
      }, current),
      open: action("mailboxOpenFile", "square-arrow-out-up-right", row => run(desktop.mailboxOpen(target(), row.item.id)), current),
      reveal: action("mailboxReveal", "folder-open", row => run(desktop.mailboxReveal(target(), row.item.id)), current),
      retry: action("mailboxRetry", "rotate-ccw", row => run(desktop.mailboxRetry(target(), row.item.id)), current),
      cancel: action("mailboxCancelItem", "x", row => run(desktop.mailboxCancel(target(), row.item.id)), current),
      remove: action("mailboxRemoveItem", "trash-2", row => run(desktop.mailboxRemove(target(), row.item.id)), current),
    };
    actionsBox.append(...Object.values(actions).map(({ root: node }) => node));
    bubble.append(text, file, bar);
    const line = document.createElement("div");
    line.className = "mailbox-line";
    line.append(bubble, actionsBox);
    root.append(line, status);
    self = { root, bubble, text, file, fileIcon, fileName, fileSize, bar, fill: fillNode, status, actions, item, iconName: "" };
    return self;
  }

  function statusText(item: MailboxItem): string {
    const values = { name, age: elapsed(item.at), done: size(item.done), total: size(item.size), folder: item.place ?? "" };
    const reason = item.error && isMessage(item.error) ? ` · ${fill(translate(item.error), values)}` : "";
    switch (item.status) {
      case "compressing": return translate("mailboxCompressing");
      case "queued": return translate("mailboxQueued");
      case "sending": return item.text !== null ? translate("mailboxQueued") : fill(translate("mailboxSending"), values);
      case "delivered": return fill(translate("mailboxDelivered"), values);
      case "waiting": return fill(translate("mailboxWaiting"), values);
      case "pickingUp": return fill(translate("mailboxPickingUp"), values);
      case "arriving": return fill(translate("mailboxArriving"), values);
      case "saved": return item.missing ? translate("mailboxMissing") : fill(translate("mailboxSaved"), values);
      case "received": return fill(translate("mailboxReceived"), values);
      case "expired": return fill(translate("mailboxExpired"), values);
      case "failed": return translate(item.mine ? "mailboxFailed" : "mailboxFailedIn") + reason;
      case "stopped": return translate("mailboxStopped");
    }
  }

  function update(row: Row, item: MailboxItem): string | undefined {
    row.item = item;
    const isText = item.text !== null;
    row.root.toggleAttribute("data-mine", item.mine);
    row.root.toggleAttribute("data-ghost", item.status === "arriving");
    row.root.toggleAttribute("data-failed", item.status === "failed");
    row.text.hidden = !isText;
    row.file.hidden = isText;
    if (isText && row.text.textContent !== item.text) row.text.textContent = item.text;
    let iconChanged: string | undefined;
    if (!isText) {
      const fileName = item.name ?? "";
      if (row.fileName.textContent !== fileName) row.fileName.textContent = fileName;
      row.fileName.title = fileName;
      const sizeText = item.folder ? fill(translate("mailboxFromFolder"), { size: size(item.size) }) : size(item.size);
      if (row.fileSize.textContent !== sizeText) row.fileSize.textContent = sizeText;
      const iconName = item.folder ? "file-archive" : /\.(png|jpe?g|gif|heic|webp)$/i.test(fileName) ? "image" : "file";
      if (row.iconName !== iconName) {
        row.iconName = iconName;
        const glyph = document.createElement("i");
        glyph.dataset.lucide = iconName;
        row.fileIcon.replaceChildren(glyph);
        iconChanged = iconName;
      }
    }
    const showBar = !isText && ["sending", "arriving", "pickingUp"].includes(item.status) && item.size > 0;
    row.bar.hidden = !showBar;
    if (showBar) row.fill.style.transform = `scaleX(${Math.min(1, item.done / item.size)})`;
    const status = statusText(item);
    if (row.status.textContent !== status) row.status.textContent = status;
    const isDevice = party !== undefined && "device" in party;
    const ended = !moving(item);
    const show = {
      copy: isText,
      open: item.status === "saved" && !item.missing,
      reveal: item.status === "saved" && !item.missing,
      retry: isDevice && (item.status === "failed" || item.status === "stopped"),
      cancel: !ended || (item.mine && item.status === "waiting"),
      remove: ended,
    };
    for (const [key, entry] of Object.entries(row.actions) as [keyof Row["actions"], Action][]) entry.root.hidden = !show[key];
    return iconChanged;
  }

  function render(): void {
    if (!conversation || !party) return;
    const languageChanged = renderedLanguage !== language();
    renderedLanguage = language();
    if (languageChanged) {
      input.placeholder = translate("mailboxWrite");
      for (const row of rows.values()) {
        for (const entry of Object.values(row.actions)) {
          const label = entry.root.dataset.tipLabel as Message;
          entry.root.dataset.tip = translate(label);
          entry.button.setAttribute("aria-label", translate(label));
        }
      }
    }
    let added = false;
    const atBottom = body.scrollHeight - body.scrollTop - body.clientHeight < 40;
    const shown = conversation.items.map(item => {
      let row = rows.get(item.id);
      if (!row) {
        row = build(item);
        rows.set(item.id, row);
        added = true;
      }
      if (update(row, item)) added = true;
      return row.root;
    });
    for (const [id, row] of rows) {
      if (!conversation.items.some(item => item.id === id)) {
        row.root.remove();
        rows.delete(id);
      }
    }
    if (shown.length !== list.children.length || shown.some((node, index) => list.children[index] !== node)) list.replaceChildren(...shown);
    empty.hidden = conversation.items.length > 0;
    clear.disabled = conversation.items.length === 0;
    // One explanation, from the last check: the mailbox is off there, the device was not reached, or it has not answered yet.
    const blocked = "device" in party ? mailboxBlocked(conversation.granted, conversation.check, name, translate, language()) : undefined;
    note.hidden = !blocked;
    if (blocked) {
      if (noteText.textContent !== blocked.text) noteText.textContent = blocked.text;
      if (noteChecked.textContent !== blocked.checked) noteChecked.textContent = blocked.checked;
    }
    const notGranted = "device" in party && conversation.granted === false;
    for (const control of [input, filesButton, folderButton, send]) control.disabled = notGranted;
    showState();
    folderText.textContent = fill(translate("mailboxSaveTo"), { folder: conversation.folder });
    folderText.title = conversation.folder;
    if (added || languageChanged) refreshIcons();
    // A new message shows, unless the person scrolled up to read.
    if (added && atBottom) body.scrollTop = body.scrollHeight;
  }

  function schedule(): void {
    window.clearTimeout(timer);
    if (!party) return;
    const busy = conversation?.items.some(moving) ?? false;
    timer = window.setTimeout(() => void refresh(), busy ? busyPoll : idlePoll);
  }

  async function refresh(): Promise<void> {
    const asked = party;
    if (!asked) return;
    const current = ++revision;
    try {
      const next = await desktop.mailboxList(asked);
      if (current !== revision || !party || !sameParty(party, asked)) return;
      conversation = next;
      render();
      // What arrives while the window is open is read.
      if (next.items.some(item => !item.read)) void desktop.mailboxMarkRead(asked).then(onChange).catch(() => {});
    } catch {
      // The window keeps what it showed. The next round reads it again.
    }
    schedule();
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
    if (open) {
      idle.hidden = false;
      confirm.hidden = true;
      error.hidden = true;
      input.focus();
    } else {
      window.clearTimeout(timer);
      window.clearInterval(liveTimer);
      party = undefined;
      conversation = undefined;
      for (const row of rows.values()) row.root.remove();
      rows.clear();
      input.value = "";
      trigger?.focus();
      onChange();
    }
  }

  /** Shows whether the device answers. */
  function showOnline(next: boolean | null): void {
    online = next;
    showState();
  }

  /** The header says whether the device answers and whether its mailbox is on, from what this device saw last. */
  function showState(): void {
    if (!party || !("device" in party)) {
      state.hidden = true;
      return;
    }
    const check = conversation?.check ?? null;
    const reachable = online !== false && check?.outcome !== "unreachable";
    const granted = conversation?.granted ?? null;
    const key: Message = !reachable ? "mailboxStateUnreachable"
      : granted === true ? "mailboxStateOn" : granted === false ? "mailboxStateOff" : "mailboxStateChecking";
    state.hidden = false;
    state.toggleAttribute("data-on", reachable && granted === true);
    const text = translate(key);
    if (state.textContent !== text) state.textContent = text;
  }

  /** While the window is open for a device that this one controls, it asks the device every 10 s and follows its state. */
  function watchDevice(watched: MailboxParty): void {
    window.clearInterval(liveTimer);
    if (!("device" in watched)) return;
    liveTimer = window.setInterval(() => {
      // Opening the same device again gives a new party object, so the watch compares by device.
      if (!party || !sameParty(party, watched)) return;
      void desktop.mailboxCheck(watched).catch(() => {}).then(() => refresh());
      void desktop.getDevices().then(overview => {
        if (!party || !sameParty(party, watched)) return;
        const device = overview.devices.find(entry => entry.id === watched.device);
        if (device) showOnline(device.online === true);
      }).catch(() => {});
    }, devicePoll);
  }

  function submit(): void {
    if (!party || !input.value.trim()) return;
    const text = input.value;
    input.value = "";
    resize();
    run(desktop.mailboxSendText(party, text));
  }

  function sendPaths(paths: string[]): void {
    if (party && paths.length) run(desktop.mailboxSendPaths(party, paths));
  }

  function resize(): void {
    input.style.height = "auto";
    input.style.height = `${Math.min(input.scrollHeight, 96)}px`;
  }

  close.addEventListener("click", () => setOpen(false));
  checkNow.addEventListener("click", () => {
    const asked = party;
    if (!asked || !("device" in asked)) return;
    checkNow.disabled = true;
    void desktop.mailboxCheck(asked).catch(() => {}).then(() => refresh()).finally(() => { checkNow.disabled = false; });
  });
  overlay.addEventListener("click", event => { if (event.target === overlay) setOpen(false); });
  overlay.addEventListener("keydown", event => { if (event.key === "Escape") setOpen(false); });
  input.addEventListener("keydown", event => {
    if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      submit();
    }
  });
  input.addEventListener("input", resize);
  send.addEventListener("click", submit);
  filesButton.addEventListener("click", () => void desktop.pickFiles().then(sendPaths).catch(code => showError(code)));
  folderButton.addEventListener("click", () => void desktop.pickFolder().then(path => path && sendPaths([path])).catch(code => showError(code)));
  changeFolder.addEventListener("click", () => {
    void desktop.pickFolder().then(path => (path ? run(desktop.mailboxSetFolder(path)) : undefined)).catch(code => showError(code));
  });
  clear.addEventListener("click", () => showConfirm(true));
  clearCancel.addEventListener("click", () => showConfirm(false));
  clearYes.addEventListener("click", () => {
    showConfirm(false);
    if (party) run(desktop.mailboxClear(party));
  });
  void desktop.onFileDrag(event => {
    if (!party) return;
    if (event.kind === "over") {
      drop.toggleAttribute("data-over", true);
      dropText.textContent = translate("mailboxDropHere");
    } else {
      drop.removeAttribute("data-over");
      dropText.textContent = translate("mailboxDrop");
      if (event.kind === "drop") sendPaths(event.paths);
    }
  });
  void desktop.onMailboxChanged(changed => {
    onChange();
    if (party && sameParty(changed, party)) void refresh();
  });

  return {
    /** Opens the conversation with a device. `online` is nothing for a device that controls this one. */
    open(next: MailboxParty, deviceName: string, deviceOnline: boolean | null, from?: HTMLElement): void {
      if (party && !sameParty(party, next)) setOpen(false);
      party = next;
      name = deviceName;
      online = deviceOnline;
      trigger = from;
      title.textContent = fill(translate("mailboxWith"), { name });
      showOnline(deviceOnline);
      renderedLanguage = undefined;
      const current = ++revision;
      // The window opens once its rows and their icons are in place, at the newest message, so nothing shows half built.
      void desktop.mailboxList(next)
        .catch((): MailboxConversation => ({ items: [], folder: "", granted: null, check: null }))
        .then(first => {
          // A read that started meanwhile may have shown newer data already, but the window still opens and starts its watch.
          if (!party || !sameParty(party, next)) return;
          if (current === revision || !conversation) conversation = first;
          render();
          setOpen(true);
          body.scrollTop = body.scrollHeight;
          schedule();
          watchDevice(next);
          void desktop.mailboxMarkRead(next).then(onChange).catch(() => {});
          // A device that this one controls is asked at once for what waits for this one.
          if ("device" in next) void desktop.mailboxCheck(next).then(() => refresh()).catch(() => {});
        });
    },
    isOpen: () => party !== undefined,
    render,
  };
}
