// Days and months in the Persian (Jalali) or Gregorian calendar, at the panel's fixed offset
// from UTC. The same arithmetic as the panel's calendar.rs (jalaali-js), so a day here is the
// day the panel counted.

export type CalendarKind = "jalali" | "gregorian";

export interface UsageSettings {
  calendar: CalendarKind;
  /** Minutes east of UTC. */
  offset: number;
}

export interface Ymd {
  y: number;
  m: number;
  d: number;
}

const UNIX_JDN = 2440588;
const BREAKS = [-61, 9, 38, 199, 426, 686, 756, 818, 1111, 1181, 1210, 1635, 2060, 2097, 2192, 2262, 2324, 2394, 2456, 3178];
const div = (a: number, b: number) => Math.trunc(a / b);
const mod = (a: number, b: number) => a - Math.trunc(a / b) * b;

function jalCal(jy: number): { leap: number; gy: number; march: number } {
  const gy = jy + 621;
  let leapJ = -14;
  let jp = BREAKS[0];
  let jump = 0;
  for (let i = 1; i < BREAKS.length; i++) {
    const jm = BREAKS[i];
    jump = jm - jp;
    if (jy < jm) break;
    leapJ += div(jump, 33) * 8 + div(mod(jump, 33), 4);
    jp = jm;
  }
  let n = jy - jp;
  leapJ += div(n, 33) * 8 + div(mod(n, 33) + 3, 4);
  if (mod(jump, 33) === 4 && jump - n === 4) leapJ += 1;
  const leapG = div(gy, 4) - div((div(gy, 100) + 1) * 3, 4) - 150;
  const march = 20 + leapJ - leapG;
  if (jump - n < 6) n = n - jump + div(jump + 4, 33) * 33;
  let leap = mod(mod(n + 1, 33) - 1, 4);
  if (leap === -1) leap = 4;
  return { leap, gy, march };
}

function g2d(gy: number, gm: number, gd: number): number {
  const d = div((gy + div(gm - 8, 6) + 100100) * 1461, 4) + div(153 * mod(gm + 9, 12) + 2, 5) + gd - 34840408;
  return d - div(div(gy + 100100 + div(gm - 8, 6), 100) * 3, 4) + 752;
}

function d2g(jdn: number): Ymd {
  let j = 4 * jdn + 139361631;
  j = j + div(div(4 * jdn + 183187720, 146097) * 3, 4) * 4 - 3908;
  const i = div(mod(j, 1461), 4) * 5 + 308;
  const d = div(mod(i, 153), 5) + 1;
  const m = mod(div(i, 153), 12) + 1;
  return { y: div(j, 1461) - 100100 + div(8 - m, 6), m, d };
}

function j2d(jy: number, jm: number, jd: number): number {
  const r = jalCal(jy);
  return g2d(r.gy, 3, r.march) + (jm - 1) * 31 - div(jm, 7) * (jm - 7) + jd - 1;
}

function d2j(jdn: number): Ymd {
  const gy = d2g(jdn).y;
  let jy = gy - 621;
  const r = jalCal(jy);
  let k = jdn - g2d(gy, 3, r.march);
  if (k >= 0) {
    if (k <= 185) return { y: jy, m: 1 + div(k, 31), d: mod(k, 31) + 1 };
    k -= 186;
  } else {
    jy -= 1;
    k += 179;
    if (r.leap === 1) k += 1;
  }
  return { y: jy, m: 7 + div(k, 30), d: mod(k, 30) + 1 };
}

/** The date of day `day` (days since 1970-01-01). */
export function dateOf(cal: CalendarKind, day: number): Ymd {
  return cal === "jalali" ? d2j(day + UNIX_JDN) : d2g(day + UNIX_JDN);
}

/** The day (since 1970-01-01) of a date. */
export function dayOf(cal: CalendarKind, y: number, m: number, d: number): number {
  return (cal === "jalali" ? j2d(y, m, d) : g2d(y, m, d)) - UNIX_JDN;
}

export function monthLen(cal: CalendarKind, y: number, m: number): number {
  if (cal === "gregorian") {
    if (m === 2) return (y % 4 === 0 && y % 100 !== 0) || y % 400 === 0 ? 29 : 28;
    return [4, 6, 9, 11].includes(m) ? 30 : 31;
  }
  if (m <= 6) return 31;
  if (m <= 11) return 30;
  return jalCal(y).leap === 0 ? 30 : 29;
}

/** Today's day in the panel's time zone. */
export function today(s: UsageSettings, now = Date.now() / 1000): number {
  return Math.floor((now + s.offset * 60) / 86400);
}

/** When a day starts, in unix seconds. */
export function dayStart(s: UsageSettings, day: number): number {
  return day * 86400 - s.offset * 60;
}

/** The day a moment (unix seconds) falls on. */
export function dayAt(s: UsageSettings, ts: number): number {
  return Math.floor((ts + s.offset * 60) / 86400);
}

/** 0 for Sunday … 6 for Saturday (1970-01-01 was a Thursday). */
export function weekday(day: number): number {
  return (((day + 4) % 7) + 7) % 7;
}

/** The first day of the week holding `day`: Saturday in the Persian calendar, Monday in the Gregorian. */
export function weekStart(cal: CalendarKind, day: number): number {
  const first = cal === "jalali" ? 6 : 1;
  return day - ((weekday(day) - first + 7) % 7);
}

export function monthStart(cal: CalendarKind, day: number): number {
  const { y, m } = dateOf(cal, day);
  return dayOf(cal, y, m, 1);
}

export function nextMonth(y: number, m: number): [number, number] {
  return m === 12 ? [y + 1, 1] : [y, m + 1];
}

export function prevMonth(y: number, m: number): [number, number] {
  return m === 1 ? [y - 1, 12] : [y, m - 1];
}

export const MONTHS: Record<CalendarKind, Record<"fa" | "en", string[]>> = {
  jalali: {
    fa: ["فروردین", "اردیبهشت", "خرداد", "تیر", "مرداد", "شهریور", "مهر", "آبان", "آذر", "دی", "بهمن", "اسفند"],
    en: ["Farvardin", "Ordibehesht", "Khordad", "Tir", "Mordad", "Shahrivar", "Mehr", "Aban", "Azar", "Dey", "Bahman", "Esfand"],
  },
  gregorian: {
    fa: ["ژانویه", "فوریه", "مارس", "آوریل", "مه", "ژوئن", "ژوئیه", "اوت", "سپتامبر", "اکتبر", "نوامبر", "دسامبر"],
    en: ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"],
  },
};

/** One-letter (fa) or short (en) weekday names, Sunday first. */
export const WEEKDAYS: Record<"fa" | "en", string[]> = {
  fa: ["ی", "د", "س", "چ", "پ", "ج", "ش"],
  en: ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"],
};
