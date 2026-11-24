// Every time the interface shows comes from this file.
// A tool can stay on for weeks, so each format keeps a bounded width at any size.

const MINUTE = 60;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

function pad(part: number): string {
  return String(part).padStart(2, "0");
}

function whole(seconds: number): number {
  return Number.isFinite(seconds) ? Math.max(0, Math.floor(seconds)) : 0;
}

/**
 * A running or remaining session: `00:14:43` under a day, `28d 04:12` from one day on.
 * Reserve `SESSION_WIDTH` characters for it.
 */
export function formatSession(seconds: number): string {
  const total = whole(seconds);
  const days = Math.floor(total / DAY);
  const hours = Math.floor(total / HOUR) % 24;
  const minutes = Math.floor(total / MINUTE) % 60;
  return days > 0 ? `${days}d ${pad(hours)}:${pad(minutes)}` : `${pad(hours)}:${pad(minutes)}:${pad(total % 60)}`;
}

/** Characters to reserve for `formatSession`. It holds a session of up to 99 days. */
export const SESSION_WIDTH = 9;

export type AgeUnit = "seconds" | "minutes" | "hours" | "days";

/** The largest whole unit of an elapsed time, so the number stays under three digits for months. */
export function age(seconds: number): { value: number; unit: AgeUnit } {
  const total = whole(seconds);
  if (total < MINUTE) return { value: total, unit: "seconds" };
  if (total < HOUR) return { value: Math.floor(total / MINUTE), unit: "minutes" };
  if (total < DAY) return { value: Math.floor(total / HOUR), unit: "hours" };
  return { value: Math.floor(total / DAY), unit: "days" };
}

/** A calendar day from an ISO date, such as `23 Nov 2026`. Its longest text has 12 characters. */
export function formatDay(isoDate: string, locale: string): string {
  const [year, month, day] = isoDate.split("-").map(Number);
  return new Date(year, month - 1, day).toLocaleDateString(locale, { day: "numeric", month: "short", year: "numeric" });
}

/**
 * The moment of an event: the time alone when it happened today, and the date with the time otherwise.
 * `now` exists for tests.
 */
export function formatMoment(epochSeconds: number, locale: string, now = new Date()): string {
  const moment = new Date(epochSeconds * 1000);
  const today = moment.getFullYear() === now.getFullYear() && moment.getMonth() === now.getMonth() &&
    moment.getDate() === now.getDate();
  return today
    ? moment.toLocaleTimeString(locale, { hour12: false })
    : moment.toLocaleString(locale, { day: "2-digit", month: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false });
}

/** True when the moment falls on the same calendar day as `now`. `now` exists for tests. */
export function isToday(epochSeconds: number, now = new Date()): boolean {
  const moment = new Date(epochSeconds * 1000);
  return moment.getFullYear() === now.getFullYear() && moment.getMonth() === now.getMonth() &&
    moment.getDate() === now.getDate();
}

/**
 * A time of day that is still to come or just passed, such as when a shell ends or an action runs next:
 * the time alone when it falls today, and the date with the time otherwise. Its longest text has 12 characters.
 * `now` exists for tests.
 */
export function formatClock(epochSeconds: number, locale: string, now = new Date()): string {
  const moment = new Date(epochSeconds * 1000);
  const today = moment.getFullYear() === now.getFullYear() && moment.getMonth() === now.getMonth() &&
    moment.getDate() === now.getDate();
  return today
    ? moment.toLocaleTimeString(locale, { hour: "2-digit", minute: "2-digit", hour12: false })
    : moment.toLocaleString(locale, { day: "2-digit", month: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false });
}

/**
 * When a file last changed: the time alone when it was today, the day and month within this year, and the date with the
 * year before that. A file can be years old. Its longest text has 10 characters (`24/11/2025`): reserve `MODIFIED_WIDTH`.
 * `now` exists for tests.
 */
export function formatModified(epochSeconds: number, locale: string, now = new Date()): string {
  const moment = new Date(epochSeconds * 1000);
  if (isToday(epochSeconds, now)) return moment.toLocaleTimeString(locale, { hour: "2-digit", minute: "2-digit", hour12: false });
  const sameYear = moment.getFullYear() === now.getFullYear();
  return moment.toLocaleDateString(locale, sameYear
    ? { day: "2-digit", month: "2-digit" } : { day: "2-digit", month: "2-digit", year: "numeric" });
}
export const MODIFIED_WIDTH = 10;

/**
 * How long a run took: `12 s` under a minute, `4 min 05 s` under an hour, and `1 h 05 min` from one hour on.
 * Its longest text has 11 characters (`59 min 59 s`) for a run that ends within an hour, as every run does.
 * A run of 100 hours or more shows whole hours, so the text stays short. Reserve `DURATION_WIDTH` characters.
 */
export function formatDuration(seconds: number): string {
  const total = whole(seconds);
  if (total < MINUTE) return `${total} s`;
  if (total < HOUR) return `${Math.floor(total / MINUTE)} min ${pad(total % MINUTE)} s`;
  const hours = Math.floor(total / HOUR);
  return hours >= 100 ? `${hours} h` : `${hours} h ${pad(Math.floor(total / MINUTE) % 60)} min`;
}

/** Characters to reserve for `formatDuration`. */
export const DURATION_WIDTH = 11;

/** A limit of time in the largest whole unit: `30 s`, `5 min`, `1 h`. A number that does not divide stays in seconds, so no text lies. */
export function formatLimit(seconds: number): string {
  const total = whole(seconds);
  if (total >= HOUR && total % HOUR === 0) return `${total / HOUR} h`;
  if (total >= MINUTE && total % MINUTE === 0) return `${total / MINUTE} min`;
  return `${total} s`;
}
