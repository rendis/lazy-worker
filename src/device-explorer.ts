import { desktop } from "./desktop";
import {
  formatBytes, joinPath, pathParts, type FileEntry, type FileShare, type SharedRoot, type Transfer,
} from "./files";
import { messages, type Language, type Message } from "./messages";
import { fadeText, only } from "./motion";
import { h, icon } from "./proxy";
import { isLinkError } from "./remote";
import { formatModified } from "./time";

// The window that opens the files that a device shares with this one: its folders, a list, and the transfers.
// Every name here comes from another device, so it is only ever set as text. The file data never reaches this window:
// the app moves it, a chunk at a time, and this window only reads the progress.

type Host = {
  translate(key: Message): string;
  language(): Language;
  refreshIcons(): void;
  /** Shows the notice of a failed change. `error` is the code that the app sent, when it sent one. */
  fail(error: unknown): void;
};

type DeviceRef = { id: string; name: string };
type Row = { entry: FileEntry; shell: HTMLElement; root: HTMLElement; size: HTMLElement; modified: HTMLElement; signature: string };

/** How often the sharing of the device is read again while the window is open, in milliseconds. A revoke shows within this time. */
const sharePoll = 5000;
/** How often the transfers are read while one runs, and while none does. */
const busyPoll = 400;
const idlePoll = 1500;
/** How long the end of a transfer shows in the bar, in milliseconds. */
const resultShown = 4000;
const closeDelay = 300;
const reduced = window.matchMedia("(prefers-reduced-motion: reduce)");
const imageTypes = new Set(["png", "jpg", "jpeg", "gif", "webp", "heic", "svg", "bmp", "tiff"]);
const textTypes = new Set(["txt", "md", "pdf", "doc", "docx", "xls", "xlsx", "csv", "json", "ts", "js", "rs", "html", "css", "pages", "numbers", "key"]);

function setText(node: HTMLElement, text: string): void {
  if (node.textContent !== text) node.textContent = text;
}

function fill(text: string, values: Record<string, string | number>): string {
  return text.replace(/\{(\w+)\}/g, (marker, key: string) => key in values ? String(values[key]) : marker);
}

/** The icon of an entry: a folder, or a file by its kind. */
function entryIcon(entry: FileEntry): string {
  if (entry.kind === "folder") return "folder";
  const extension = entry.name.includes(".") ? entry.name.split(".").pop()!.toLowerCase() : "";
  return imageTypes.has(extension) ? "file-image" : textTypes.has(extension) ? "file-text" : "file";
}

export function mountFilesExplorer(host: Host) {
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

  /** An icon-only button with its name as a tip. The wrapper carries the tip, so a disabled button can still give a reason. */
  function iconButton(key: Message, name: string, extra = "") {
    const button = h("button", { type: "button", class: `icon-button ${extra}`.trim() }, icon(name));
    const root = h("span", { class: "tip" }, button);
    let reason: string | undefined;
    const apply = (): void => {
      const label = t(key);
      root.setAttribute("data-tip", reason ?? label);
      button.setAttribute("aria-label", label);
    };
    apply();
    labelled.push(apply);
    return {
      root,
      button,
      /** Turns the button off with the reason as its tip, or back on with its name. */
      disable(why: string | undefined): void {
        reason = why;
        const off = why !== undefined;
        if (button.disabled !== off) button.disabled = off;
        apply();
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

  const overlay = document.getElementById("files-explorer-window")!;
  const title = document.getElementById("files-explorer-title")!;
  const content = document.getElementById("files-explorer-body")!;
  const close = document.getElementById("files-explorer-close") as HTMLButtonElement;

  let device: DeviceRef | undefined;
  let trigger: HTMLElement | undefined;
  let share: FileShare | undefined;
  let rootId: string | undefined;
  let path = "";
  let entries: FileEntry[] = [];
  let truncated = false;
  let loaded = false;
  let sortAscending = true;
  let selected = new Set<string>();
  let anchor: string | undefined;
  let confirmingDelete = false;
  let creating = false;
  let reading = 0;
  let status = "";
  let sharePoller: number | undefined;
  let transferPoller: number | undefined;
  let baseline = 0;
  let dragging = false;
  const handled = new Set<number>();
  let result: { text: string; failed: boolean; until: number } | undefined;

  // ---- The parts of the window ----
  const crumbs = h("nav", { class: "files-crumbs" });
  bind(crumbs, "filesPath", "aria-label");
  const upload = iconButton("filesUpload", "upload", "outlined");
  const makeFolder = iconButton("filesNewFolder", "folder-plus", "outlined");
  const download = iconButton("filesDownload", "download", "outlined");
  const remove = iconButton("filesDelete", "trash-2", "outlined");
  const refresh = iconButton("filesRefresh", "refresh-cw", "outlined");
  const deleteText = h("span", { class: "setting-note files-confirm-text" });
  const deleteAccept = iconButton("filesDeleteAccept", "check", "danger");
  const deleteCancel = iconButton("cancel", "x");
  const toolbar = h("div", { class: "device-end files-tools" }, upload.root, makeFolder.root, download.root, remove.root, refresh.root);
  const confirm = h("div", { class: "device-confirm files-confirm", hidden: true }, deleteText, deleteAccept.root, deleteCancel.root);
  const head = h("div", { class: "files-head" }, crumbs, toolbar, confirm);

  const rootList = h("div", { class: "files-roots-list" });
  const rootsTitle = bind(h("h3", { class: "files-roots-title" }), "filesRoots");
  const roots = h("aside", { class: "files-roots" }, rootsTitle, rootList);

  const nameSort = h("button", { type: "button", class: "files-sort" }, bind(h("span"), "filesColName"), icon("chevron-down"));
  const columns = h("div", { class: "files-columns" }, nameSort,
    bind(h("span", { class: "files-col-size" }), "filesColSize"), bind(h("span", { class: "files-col-modified" }), "filesColModified"));
  const newName = h("input", { class: "field files-new-name", type: "text", maxlength: "255", autocomplete: "off", spellcheck: "false" });
  bind(newName, "filesNewFolderName", "aria-label");
  bind(newName, "filesNewFolderName", "placeholder");
  const newCreate = iconButton("filesNewFolderCreate", "check", "outlined");
  const newCancel = iconButton("cancel", "x");
  const newBox = collapse(h("div", { class: "files-new" }, icon("folder-plus"), newName, newCreate.root, newCancel.root));
  const rowsList = h("ul", { class: "files-rows", role: "listbox", tabindex: "0", "aria-multiselectable": "true" });
  bind(rowsList, "filesRoots", "aria-label");
  const message = h("p", { class: "setting-note files-message", role: "status" });
  const listBox = h("div", { class: "files-list" }, columns, newBox.root, rowsList, message);

  const drop = h("div", { class: "files-drop" }, icon("upload"), h("span"));
  const barLabel = h("span", { class: "files-bar-label" });
  const barQueue = h("span", { class: "setting-note files-bar-queue" });
  const barFill = h("span", { class: "files-bar-fill" });
  const stop = iconButton("filesStopTransfer", "square", "outlined");
  const transferBox = collapse(h("div", { class: "files-transfer" },
    h("div", { class: "files-transfer-text" },
      h("div", { class: "files-transfer-line" }, barLabel, barQueue),
      h("div", { class: "files-bar-track", role: "progressbar", "aria-valuemin": "0", "aria-valuemax": "100" }, barFill)),
    stop.root));

  const main = h("div", { class: "files-main" }, roots, listBox);
  const body = h("div", { class: "files-shell" }, head, main, drop, transferBox.root);
  content.replaceChildren(body);
  const notShared = h("p", { class: "setting-note files-unshared" });
  content.append(notShared);

  // ---- What the window knows ----
  /** A chosen folder shows its name. The home folder and the whole disk have none, so they show their own words. */
  function rootName(root: SharedRoot): string {
    return root.name || t(root.id === "home" ? "filesScopeHomeRoot" : "filesScopeDisk");
  }

  function current(): SharedRoot | undefined {
    return share?.on ? share.roots.find(root => root.id === rootId) : undefined;
  }
  const writable = (): boolean => share?.on === true && share.mode === "readWrite";

  function errorText(error: unknown): string {
    if (typeof error === "string" && isLinkError(error)) return t("remoteLost");
    if (error === "notPermitted" || error === "localOnly") return fill(t("notPermitted"), { name: device?.name ?? "" });
    return typeof error === "string" && Object.prototype.hasOwnProperty.call(messages.en, error) ? t(error as Message) : t("filesIo");
  }

  function sorted(): FileEntry[] {
    const direction = sortAscending ? 1 : -1;
    return [...entries].sort((a, b) =>
      (a.kind === b.kind ? 0 : a.kind === "folder" ? -1 : 1)
      || direction * a.name.localeCompare(b.name, host.language(), { numeric: true, sensitivity: "base" }));
  }

  // ---- The list: one element per row, updated in place ----
  const rows = new Map<string, Row>();
  let rendered: string | undefined;

  function buildRow(entry: FileEntry): Row {
    const size = h("span", { class: "files-size" });
    const modified = h("span", { class: "files-modified" });
    const root = h("li", { class: "files-row", role: "option", "aria-selected": "false", "data-kind": entry.kind },
      h("span", { class: "files-name" }, icon(entryIcon(entry)), h("span", { class: "files-name-text" }, entry.name)),
      size, modified);
    const shell = h("div", { class: "collapse" }, h("div", {}, root));
    root.addEventListener("click", event => select(entry.name, event));
    root.addEventListener("dblclick", () => openEntry(entry));
    return { entry, shell, root, size, modified, signature: "" };
  }

  function renderList(): void {
    const items = sorted();
    // A change of folder or of language swaps the rows at once. A refresh keeps them and moves only what changed.
    const scope = JSON.stringify([rootId, path, host.language()]);
    const swapped = rendered !== scope;
    rendered = scope;
    const animate = !swapped && !reduced.matches && rowsList.isConnected && !overlay.hasAttribute("inert");
    if (swapped) {
      rows.clear();
      rowsList.replaceChildren();
    }
    const seen = new Set<string>();
    let previous: HTMLElement | undefined;
    for (const entry of items) {
      seen.add(entry.name);
      let row = rows.get(entry.name);
      if (row && (row.entry.kind !== entry.kind)) {
        row.shell.remove();
        rows.delete(entry.name);
        row = undefined;
      }
      let created = false;
      if (!row) {
        row = buildRow(entry);
        rows.set(entry.name, row);
        created = true;
      }
      row.entry = entry;
      setText(row.size, entry.kind === "folder" ? "" : formatBytes(entry.size, host.language()));
      setText(row.modified, formatModified(entry.modified, host.language()));
      row.root.setAttribute("aria-selected", String(selected.has(entry.name)));
      row.root.toggleAttribute("data-selected", selected.has(entry.name));
      const expected = previous ? previous.nextElementSibling : rowsList.firstElementChild;
      if (row.shell !== expected) rowsList.insertBefore(row.shell, expected);
      if (created) {
        // A new row starts closed and opens on the next frame, so the rows under it move instead of jumping.
        if (animate) void row.shell.offsetHeight;
        row.shell.setAttribute("data-open", "");
      }
      previous = row.shell;
    }
    for (const [name, row] of rows) {
      if (seen.has(name)) continue;
      rows.delete(name);
      row.shell.removeAttribute("data-open");
      row.shell.setAttribute("inert", "");
      if (animate) window.setTimeout(() => row.shell.remove(), closeDelay);
      else row.shell.remove();
    }
    host.refreshIcons();
  }

  function renderCrumbs(): void {
    const root = current();
    if (!root) {
      crumbs.replaceChildren();
      return;
    }
    const parts = pathParts(path);
    const signature = JSON.stringify([root.id, root.name, parts, host.language()]);
    if (crumbs.dataset.signature === signature) return;
    crumbs.dataset.signature = signature;
    const nodes: HTMLElement[] = [];
    const crumb = (label: string, to: string, last: boolean): HTMLElement => {
      if (last) return h("strong", { class: "files-crumb", "aria-current": "page" }, label);
      const button = h("button", { type: "button", class: "files-crumb" }, label);
      button.addEventListener("click", () => go(to));
      return button;
    };
    nodes.push(crumb(rootName(root), "", parts.length === 0));
    parts.forEach((part, index) => {
      nodes.push(icon("chevron-right"), crumb(part, parts.slice(0, index + 1).join("/"), index === parts.length - 1));
    });
    crumbs.replaceChildren(...nodes);
    host.refreshIcons();
  }

  function renderRoots(): void {
    const items = share?.on ? share.roots : [];
    const signature = JSON.stringify([items, rootId, host.language()]);
    if (rootList.dataset.signature === signature) return;
    rootList.dataset.signature = signature;
    rootList.replaceChildren(...items.map(root => {
      const label = rootName(root);
      const button = h("button", {
        type: "button", class: "files-root", "aria-pressed": String(root.id === rootId), "data-tip": root.display,
      }, icon(root.name === "" ? "hard-drive" : "folder"), h("span", { class: "files-root-name" }, label));
      button.addEventListener("click", () => {
        if (root.id === rootId) go("");
        else void choose(root.id);
      });
      return h("span", { class: "tip files-root-tip", "data-tip": root.display }, button);
    }));
    host.refreshIcons();
  }

  /** What shows where the rows would: reading, empty, or the reason that nothing shows. */
  function renderMessage(): void {
    let text = status;
    if (!text && reading > 0 && !loaded) text = t("filesLoading");
    if (!text && loaded && entries.length === 0) text = t("filesEmpty");
    if (!text && truncated) text = t("filesTruncated");
    if (text !== "") fadeText(message, text);
    else setText(message, "");
  }

  function renderButtons(): void {
    const files = selectedEntries();
    const readOnly = t("filesReadOnlyReason");
    upload.disable(!current() ? readOnly : writable() ? undefined : readOnly);
    makeFolder.disable(!current() ? readOnly : writable() ? undefined : readOnly);
    download.disable(files.length === 0 ? t("filesSelectFirst") : files.some(entry => entry.kind === "folder") ? t("filesSelectFiles") : undefined);
    remove.disable(!writable() ? readOnly : files.length === 0 ? t("filesSelectFirst") : undefined);
    refresh.disable(current() ? undefined : t("filesNoRoots"));
    only([toolbar, confirm], confirmingDelete ? [confirm] : [toolbar]);
    if (confirmingDelete) {
      const picked = selectedEntries();
      setText(deleteText, picked.length > 1 ? fill(t("filesDeleteMany"), { count: picked.length })
        : picked[0]?.kind === "folder" ? fill(t("filesDeleteFolder"), { name: picked[0].name })
        : fill(t("filesDeleteOne"), { name: picked[0]?.name ?? "" }));
    }
    nameSort.setAttribute("aria-label", t("filesColName"));
    nameSort.dataset.direction = sortAscending ? "ascending" : "descending";
    const dropText = !current() ? "" : writable()
      ? fill(t("filesDrop"), { folder: path === "" ? (current() ? rootName(current()!) : t("filesScopeDisk")) : pathParts(path).slice(-1)[0] })
      : t("filesDropReadOnly");
    const dropSpan = drop.querySelector("span")!;
    if (dropText !== "") fadeText(dropSpan, dropText);
    drop.toggleAttribute("data-off", !writable());
    drop.toggleAttribute("data-over", dragging && writable());
  }

  function render(): void {
    if (!device) return;
    setText(title, fill(t("filesTitle"), { name: device.name }));
    const off = share !== undefined && !share.on;
    body.hidden = off;
    notShared.hidden = !off;
    if (off) setText(notShared, fill(t("filesNotShared"), { name: device.name }));
    renderRoots();
    renderCrumbs();
    renderList();
    renderMessage();
    renderButtons();
    newBox.set(creating && writable());
    const name = newName.value.trim();
    newCreate.disable(name === "" || name.includes("/") || name === "." || name === ".." || new TextEncoder().encode(name).length > 255
      ? t("filesPathInvalid") : undefined);
  }

  function selectedEntries(): FileEntry[] {
    return entries.filter(entry => selected.has(entry.name));
  }

  function select(name: string, event?: MouseEvent): void {
    const order = sorted().map(entry => entry.name);
    if (event?.shiftKey && anchor !== undefined && order.includes(anchor)) {
      const [from, to] = [order.indexOf(anchor), order.indexOf(name)].sort((a, b) => a - b);
      selected = new Set(order.slice(from, to + 1));
    } else if (event?.metaKey || event?.ctrlKey) {
      selected = new Set(selected);
      if (!selected.delete(name)) selected.add(name);
      anchor = name;
    } else {
      selected = new Set([name]);
      anchor = name;
    }
    confirmingDelete = false;
    render();
  }

  // ---- Reading the device ----
  async function readShare(): Promise<void> {
    if (!device) return;
    const id = device.id;
    try {
      const next = await desktop.remoteFileShare(id);
      if (!device || device.id !== id) return;
      const before = share;
      share = next;
      if (!next.on) {
        rootId = undefined;
        entries = [];
        loaded = false;
        render();
        return;
      }
      const stillThere = next.roots.some(root => root.id === rootId);
      if (!stillThere) {
        // The first folder opens when the window opens, and when the one that was open is not shared any more.
        rootId = next.roots[0]?.id;
        path = "";
        entries = [];
        loaded = false;
        selected = new Set();
        status = rootId === undefined ? t("filesNoRoots") : "";
        if (rootId !== undefined) void load();
      } else if (before?.on === false) void load();
      render();
    } catch (error) {
      if (!device) return;
      status = errorText(error);
      render();
    }
  }

  async function load(): Promise<void> {
    const open = device;
    const root = rootId;
    if (!open || root === undefined) return;
    const asked = path;
    reading++;
    renderMessage();
    try {
      const listing = await desktop.remoteListFiles(open.id, root, asked);
      if (device !== open || rootId !== root || path !== asked) return;
      entries = listing.entries;
      truncated = listing.truncated;
      loaded = true;
      status = "";
      // What is selected must still be there.
      selected = new Set([...selected].filter(name => entries.some(entry => entry.name === name)));
    } catch (error) {
      if (device !== open || rootId !== root || path !== asked) return;
      // A folder that went away takes the window back to the shared folder. Any other failure stays in the list's place.
      if (error === "filesNotFound" && path !== "") {
        path = "";
        selected = new Set();
        reading--;
        void load();
        reading++;
      } else {
        status = errorText(error);
        entries = [];
        loaded = true;
      }
    } finally {
      reading--;
    }
    render();
  }

  function go(to: string): void {
    path = to;
    selected = new Set();
    anchor = undefined;
    confirmingDelete = false;
    creating = false;
    loaded = false;
    status = "";
    render();
    void load();
  }

  async function choose(id: string): Promise<void> {
    rootId = id;
    path = "";
    entries = [];
    go("");
  }

  function openEntry(entry: FileEntry): void {
    if (entry.kind === "folder") go(joinPath(path, entry.name));
  }

  // ---- What the buttons do ----
  function startUpload(files: string[]): void {
    const root = current();
    if (!device || !root || !writable() || files.length === 0) return;
    void desktop.queueUpload(device.id, root.id, path, files).then(() => readTransfers(), (error: unknown) => host.fail(error));
  }

  upload.button.addEventListener("click", () => {
    void desktop.pickFiles().then(files => startUpload(files), (error: unknown) => host.fail(error));
  });

  download.button.addEventListener("click", () => {
    const files = selectedEntries().filter(entry => entry.kind === "file");
    const root = current();
    if (!device || !root || files.length === 0) return;
    const paths = files.map(entry => joinPath(path, entry.name));
    // One file asks where to save it. Several ask for a folder, and keep their names.
    const target = files.length === 1 ? desktop.pickSaveFile(files[0].name) : desktop.pickFolder();
    void target.then(destination => {
      if (destination !== null) return desktop.queueDownload(device!.id, root.id, paths, destination).then(() => readTransfers());
    }).catch((error: unknown) => host.fail(error));
  });

  refresh.button.addEventListener("click", () => {
    void readShare();
    void load();
  });

  remove.button.addEventListener("click", () => {
    confirmingDelete = true;
    render();
    deleteCancel.button.focus();
  });
  deleteCancel.button.addEventListener("click", () => {
    confirmingDelete = false;
    render();
    remove.button.focus();
  });
  deleteAccept.button.addEventListener("click", () => {
    const root = current();
    const doomed = selectedEntries();
    if (!device || !root) return;
    const id = device.id;
    confirmingDelete = false;
    render();
    void (async () => {
      for (const entry of doomed) {
        await desktop.remoteDeleteEntry(id, root.id, joinPath(path, entry.name), entry.kind === "folder");
      }
    })().catch((error: unknown) => host.fail(error)).finally(() => {
      selected = new Set();
      void load();
      remove.button.focus();
    });
  });

  makeFolder.button.addEventListener("click", () => {
    creating = true;
    newName.value = "";
    render();
    newName.focus();
  });
  newCancel.button.addEventListener("click", () => {
    creating = false;
    render();
    makeFolder.button.focus();
  });
  newName.addEventListener("input", render);
  newName.addEventListener("keydown", event => {
    if (event.key === "Escape") {
      event.stopPropagation();
      newCancel.button.click();
    }
  });
  const createFolder = (): void => {
    const root = current();
    const name = newName.value.trim();
    if (!device || !root || newCreate.button.disabled) return;
    void desktop.remoteMakeFolder(device.id, root.id, path, name).then(() => {
      creating = false;
      void load();
      makeFolder.button.focus();
    }, (error: unknown) => host.fail(error));
  };
  newCreate.button.addEventListener("click", createFolder);
  newName.addEventListener("keydown", event => {
    if (event.key === "Enter") {
      event.preventDefault();
      createFolder();
    }
  });

  nameSort.addEventListener("click", () => {
    sortAscending = !sortAscending;
    render();
  });

  rowsList.addEventListener("keydown", event => {
    const order = sorted();
    const index = order.findIndex(entry => entry.name === anchor);
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const next = order[Math.min(order.length - 1, Math.max(0, index + (event.key === "ArrowDown" ? 1 : -1)))];
      if (next) {
        select(next.name);
        rows.get(next.name)?.root.scrollIntoView({ block: "nearest" });
      }
    } else if (event.key === "Enter" && index >= 0) {
      openEntry(order[index]);
    } else if (event.key === "Backspace" && path !== "") {
      event.preventDefault();
      go(pathParts(path).slice(0, -1).join("/"));
    }
  });

  // ---- Transfers ----
  function mine(jobs: Transfer[]): Transfer[] {
    return jobs.filter(job => device !== undefined && job.device === device.id && job.id > baseline);
  }

  function renderTransfer(jobs: Transfer[]): void {
    const active = jobs.filter(job => job.state === "queued" || job.state === "running");
    const running = active.find(job => job.state === "running") ?? active[0];
    if (running) {
      const label = fill(t(running.kind === "upload" ? "filesSendingName" : "filesSavingName"), { name: running.name });
      const progress = fill(t("filesProgress"), {
        done: formatBytes(running.done, host.language()), total: formatBytes(running.total, host.language()),
      });
      setText(barLabel, `${label} · ${progress}`);
      barLabel.toggleAttribute("data-warning", false);
      setText(barQueue, active.length > 1 ? fill(t("filesMoreWaiting"), { count: active.length - 1 }) : "");
      const percent = running.total === 0 ? 0 : Math.min(100, (running.done / running.total) * 100);
      barFill.style.transform = `scaleX(${percent / 100})`;
      barFill.parentElement!.setAttribute("aria-valuenow", String(Math.round(percent)));
      stop.disable(undefined);
      stop.button.onclick = () => void desktop.stopTransfer(running.id).then(() => readTransfers());
      transferBox.set(true);
      return;
    }
    // After the last one ends, its result shows for a few seconds.
    if (result && result.until > Date.now()) {
      setText(barLabel, result.text);
      barLabel.toggleAttribute("data-warning", result.failed);
      setText(barQueue, "");
      barFill.style.transform = `scaleX(${result.failed ? 0 : 1})`;
      stop.disable(t("filesNothingMoving"));
      transferBox.set(true);
    } else {
      transferBox.set(false);
    }
  }

  async function readTransfers(): Promise<void> {
    window.clearTimeout(transferPoller);
    transferPoller = undefined;
    if (!device) return;
    let jobs: Transfer[] = [];
    try {
      jobs = mine(await desktop.getTransfers());
    } catch {
      // The next read tries again.
    }
    if (!device) return;
    let reload = false;
    for (const job of jobs) {
      if (job.state === "queued" || job.state === "running" || handled.has(job.id)) continue;
      handled.add(job.id);
      if (job.state === "done" && job.kind === "upload") reload = true;
      const text = job.state === "done" ? fill(t("filesTransferDone"), { name: job.name })
        : job.state === "stopped" ? fill(t("filesTransferStopped"), { name: job.name })
        : fill(t("filesTransferFailed"), { name: job.name, reason: errorText(job.error) });
      result = { text, failed: job.state === "failed", until: Date.now() + resultShown };
    }
    renderTransfer(jobs);
    if (reload) void load();
    const busy = jobs.some(job => job.state === "queued" || job.state === "running");
    transferPoller = window.setTimeout(() => void readTransfers(), busy || (result && result.until > Date.now()) ? busyPoll : idlePoll);
  }

  // ---- Files dragged from the Finder ----
  void desktop.onFileDrag(event => {
    if (!device || !overlay.hasAttribute("data-open")) return;
    if (event.kind === "over") dragging = true;
    else if (event.kind === "leave") dragging = false;
    else {
      dragging = false;
      startUpload(event.paths);
    }
    renderButtons();
  });

  // ---- Opening and closing ----
  function open(next: DeviceRef, from: HTMLElement): void {
    device = next;
    trigger = from;
    share = undefined;
    rootId = undefined;
    path = "";
    entries = [];
    truncated = false;
    loaded = false;
    selected = new Set();
    anchor = undefined;
    confirmingDelete = false;
    creating = false;
    status = "";
    result = undefined;
    handled.clear();
    rendered = undefined;
    delete crumbs.dataset.signature;
    delete rootList.dataset.signature;
    rows.clear();
    rowsList.replaceChildren();
    // Transfers that ended before the window opened are not its own.
    baseline = 0;
    void desktop.getTransfers().then(jobs => {
      baseline = jobs.reduce((top, job) => Math.max(top, job.state === "queued" || job.state === "running" ? 0 : job.id), 0);
      void readTransfers();
    }, () => void readTransfers());
    render();
    overlay.setAttribute("data-open", "");
    overlay.inert = false;
    window.clearInterval(sharePoller);
    sharePoller = window.setInterval(() => void readShare(), sharePoll);
    void readShare();
    host.refreshIcons();
    rowsList.focus();
  }

  function closeWindow(restoreFocus = true): void {
    if (!overlay.hasAttribute("data-open")) return;
    overlay.removeAttribute("data-open");
    overlay.inert = true;
    window.clearInterval(sharePoller);
    window.clearTimeout(transferPoller);
    sharePoller = undefined;
    transferPoller = undefined;
    device = undefined;
    dragging = false;
    if (restoreFocus && trigger?.isConnected) trigger.focus();
  }

  close.addEventListener("click", () => closeWindow());
  overlay.addEventListener("pointerdown", event => {
    if (event.target === overlay) closeWindow();
  });

  return {
    open,
    /** Closes the window when it is open. It returns true when it was. */
    closeTop(): boolean {
      if (!overlay.hasAttribute("data-open")) return false;
      // The first press of Escape leaves what is open inside the window, such as a question or a new folder.
      if (confirmingDelete || creating) {
        confirmingDelete = false;
        creating = false;
        render();
        return true;
      }
      closeWindow();
      return true;
    },
    closeAll(): void {
      closeWindow(false);
    },
    /** Puts the words of the window in the language of the interface. */
    retranslate(): void {
      for (const apply of labelled) apply();
      delete crumbs.dataset.signature;
      delete rootList.dataset.signature;
      render();
    },
  };
}
