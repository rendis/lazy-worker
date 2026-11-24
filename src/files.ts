// The files that devices share: the shapes that cross the app command, how sizes read, and the sample files a browser preview shows.

export type FilesMode = "read" | "readWrite";
/** The folders the person chose, the home folder, or the whole disk (`/`). */
export type FilesScope = "folders" | "home" | "disk";
/** What reading a place that macOS closes behind a privacy question showed. */
export type PrivacyTouch = "readable" | "denied" | "pending" | "failed";
/** A protected place that the app read when the grant was saved. `path` shows `~` for the home folder. */
export type PrivacyCheck = { path: string; touch: PrivacyTouch };
/** When a grant ends: seconds since the Unix epoch, or `"untilOff"` while it lasts until the person turns it off. */
export type FilesUntil = number | "untilOff";
/** The files that this device shared with a device, as the list of devices that control this one reads it. The folders show with `~`. */
export type FilesGrant = { scope: FilesScope; folders: string[]; mode: FilesMode; until: FilesUntil };
/** What saving a grant answers: the grant, and what each protected place inside it showed when the app read it. */
export type FilesGranted = FilesGrant & { privacy: PrivacyCheck[] };
export type SharedRoot = { id: string; name: string; display: string };
/** What the device that is controlled shares with this one. `roots` has one folder with no name for the home folder or the whole disk. */
export type FileShare =
  | { on: false }
  | { on: true; scope: FilesScope; mode: FilesMode; until: FilesUntil; roots: SharedRoot[] };
export type FileEntry = { name: string; kind: "file" | "folder"; size: number; modified: number };
export type FileListing = { entries: FileEntry[]; truncated: boolean };
export type TransferState = "queued" | "running" | "done" | "failed" | "stopped";
/** One transfer. `error` is an error word when it failed. */
export type Transfer = {
  id: number;
  device: string;
  kind: "upload" | "download";
  name: string;
  /** True for a download that opens with its default application when it ends. */
  open: boolean;
  total: number;
  done: number;
  state: TransferState;
  error: string | null;
};

/** The characters that a size takes at most, for a column that must not move. */
export const SIZE_WIDTH = 8;
const units = ["B", "KB", "MB", "GB", "TB"];

/**
 * A size in bytes with its unit, such as `24 KB` or `1.2 MB`: one decimal under 10, none above.
 * The comma follows the language. Its longest text has 6 characters (`999 KB`) for any file under a petabyte: reserve `SIZE_WIDTH`.
 */
export function formatBytes(bytes: number, locale: string): string {
  let value = Math.max(0, bytes);
  let unit = 0;
  while (value >= 999.5 && unit < units.length - 1) {
    value /= 1000;
    unit++;
  }
  const digits = unit === 0 || value >= 10 ? 0 : 1;
  return `${value.toLocaleString(locale, { maximumFractionDigits: digits, minimumFractionDigits: digits })} ${units[unit]}`;
}

/** A path inside a shared folder as its names, from `""` for the folder itself. */
export function pathParts(path: string): string[] {
  return path === "" ? [] : path.split("/");
}

/** The path of a name inside a folder. */
export function joinPath(folder: string, name: string): string {
  return folder === "" ? name : `${folder}/${name}`;
}

// ---- The sample files of a preview ----

type SampleFolder = { [name: string]: SampleFolder | number };

/** The sample shares of the device `studio`: a folder is an object, and a file is its size in bytes. */
const sampleRoots: Record<string, { name: string; display: string; tree: SampleFolder }> = {
  work: {
    name: "Lazy Worker", display: "~/Lazy Worker",
    tree: { Clips: { "demo.mp4": 18_400_000, "poster.png": 420_000 }, "notas.txt": 1_200, "presupuesto.xlsx": 56_000 },
  },
  proj: {
    name: "Proyectos", display: "~/Documents/Proyectos",
    tree: {
      "lazy-worker": {
        docs: { "user-guide.md": 24_000, "development.md": 8_000, "design.png": 1_200_000 },
        src: { "main.ts": 14_000, "devices.ts": 120_000 },
        "README.md": 6_400,
      },
      archivo: {},
    },
  },
  home: { name: "", display: "~", tree: { Documents: { "cv.pdf": 210_000 }, Desktop: {}, Downloads: {}, "Lazy Worker": {} } },
  disk: { name: "", display: "/", tree: { Users: { alex: { Documents: { "cv.pdf": 210_000 }, Desktop: {} } }, Applications: {}, tmp: {} } },
};
const sampleTimes = new Map<string, number>();

export function previewShareRoots(scope: FilesScope): SharedRoot[] {
  const ids = scope === "disk" ? ["disk"] : scope === "home" ? ["home"] : ["work", "proj"];
  return ids.map(id => ({ id, name: sampleRoots[id].name, display: sampleRoots[id].display }));
}

function sampleAt(root: string, path: string): SampleFolder | number | undefined {
  let found: SampleFolder | number | undefined = sampleRoots[root]?.tree;
  for (const part of pathParts(path)) {
    if (typeof found !== "object") return undefined;
    found = found[part];
  }
  return found;
}

function sampleTime(key: string): number {
  if (!sampleTimes.has(key)) {
    let hash = 0;
    for (const letter of key) hash = (hash * 31 + letter.charCodeAt(0)) % 7_000_000;
    sampleTimes.set(key, Math.floor(Date.now() / 1000) - 1800 - hash);
  }
  return sampleTimes.get(key)!;
}

/** Lists a sample folder. A path that is not a folder fails as the device would. */
export function previewList(root: string, path: string): FileListing {
  const folder = sampleAt(root, path);
  if (folder === undefined) throw "filesNotFound";
  if (typeof folder !== "object") throw "filesNotFolder";
  const entries = Object.entries(folder).map(([name, value]): FileEntry => ({
    name, kind: typeof value === "object" ? "folder" : "file", size: typeof value === "number" ? value : 0,
    modified: sampleTime(`${root}/${joinPath(path, name)}`),
  }));
  entries.sort((a, b) => (a.kind === b.kind ? 0 : a.kind === "folder" ? -1 : 1) || a.name.localeCompare(b.name));
  return { entries, truncated: false };
}

function sampleParent(root: string, path: string): { folder: SampleFolder; name: string } {
  const parts = pathParts(path);
  const name = parts.pop();
  const folder = sampleAt(root, parts.join("/"));
  if (name === undefined) throw "filesPathInvalid";
  if (typeof folder !== "object") throw "filesNotFound";
  return { folder, name };
}

export function previewMakeFolder(root: string, path: string, name: string): void {
  const folder = sampleAt(root, path);
  if (typeof folder !== "object") throw "filesNotFound";
  if (name === "" || name.includes("/") || name === "." || name === "..") throw "filesPathInvalid";
  if (name in folder) throw "filesExists";
  folder[name] = {};
  sampleTimes.set(`${root}/${joinPath(path, name)}`, Math.floor(Date.now() / 1000));
}

export function previewDelete(root: string, path: string, recursive: boolean): void {
  const { folder, name } = sampleParent(root, path);
  const found = folder[name];
  if (found === undefined) throw "filesNotFound";
  if (typeof found === "object" && !recursive && Object.keys(found).length > 0) throw "filesNotEmpty";
  delete folder[name];
}

/** Gives a sample entry a new name in its folder. A name that is taken is refused, as the device refuses it. */
export function previewRename(root: string, path: string, name: string): void {
  const { folder, name: old } = sampleParent(root, path);
  if (folder[old] === undefined) throw "filesNotFound";
  if (name === "" || name.includes("/") || name === "." || name === "..") throw "filesPathInvalid";
  if (name === old) return;
  if (name in folder) throw "filesExists";
  const parent = pathParts(path).slice(0, -1).join("/");
  const entries = Object.entries(folder);
  // The entry keeps its place in the object, so a refresh lists what it listed before.
  for (const [key] of entries) delete folder[key];
  for (const [key, value] of entries) folder[key === old ? name : key] = value;
  sampleTimes.set(`${root}/${joinPath(parent, name)}`, sampleTime(`${root}/${path}`));
}

export function previewAddFile(root: string, path: string, name: string, size: number): void {
  const folder = sampleAt(root, path);
  if (typeof folder !== "object") throw "filesNotFound";
  folder[name] = size;
  sampleTimes.set(`${root}/${joinPath(path, name)}`, Math.floor(Date.now() / 1000));
}

export function previewFileSize(root: string, path: string): number {
  const found = sampleAt(root, path);
  if (typeof found !== "number") throw "filesNotFound";
  return found;
}

// ---- The sample transfers of a preview: one at a time, at a steady speed ----

const previewSpeed = 3_000_000;
type PreviewJob = Transfer & { onDone: () => void; runningSince: number | null };
const previewJobs: PreviewJob[] = [];
let previewJobId = 1;

/** Moves the queue on to the clock, as the worker of the app would. */
function previewAdvance(): void {
  const now = performance.now();
  for (;;) {
    const job = previewJobs.find(item => item.state === "queued" || item.state === "running");
    if (!job) return;
    if (job.state === "queued") {
      job.state = "running";
      job.runningSince = now;
    }
    job.done = Math.min(job.total, Math.floor(((now - (job.runningSince ?? now)) / 1000) * previewSpeed));
    if (job.done < job.total) return;
    // The next one begins where this one ended, so a long wait between two reads loses no time.
    const ended = (job.runningSince ?? now) + (job.total / previewSpeed) * 1000;
    job.state = "done";
    job.onDone();
    const next = previewJobs.find(item => item.state === "queued");
    if (next) next.runningSince = ended;
    if (!next) return;
    next.state = "running";
  }
}

export function previewQueue(kind: Transfer["kind"], device: string, name: string, total: number, onDone: () => void, open = false): void {
  previewJobs.push({
    id: previewJobId++, device, kind, name, open, total, done: 0, state: "queued", error: null, onDone, runningSince: null,
  });
  previewAdvance();
}

export function previewTransfers(): Transfer[] {
  previewAdvance();
  return previewJobs.map(({ onDone: _onDone, runningSince: _since, ...job }) => ({ ...job }));
}

export function previewStopTransfer(id: number): void {
  const job = previewJobs.find(item => item.id === id);
  if (job && (job.state === "queued" || job.state === "running")) job.state = "stopped";
  previewAdvance();
}

export function previewClearTransfers(): void {
  for (let index = previewJobs.length - 1; index >= 0; index--) {
    if (previewJobs[index].state !== "queued" && previewJobs[index].state !== "running") previewJobs.splice(index, 1);
  }
}
