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
