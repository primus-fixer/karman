// Суммы хранятся в копейках, даты — строками ГГГГ-ММ-ДД.

export const NB = ' ';
const GEN = ['января', 'февраля', 'марта', 'апреля', 'мая', 'июня', 'июля', 'августа', 'сентября', 'октября', 'ноября', 'декабря'];
const NOM = ['Январь', 'Февраль', 'Март', 'Апрель', 'Май', 'Июнь', 'Июль', 'Август', 'Сентябрь', 'Октябрь', 'Ноябрь', 'Декабрь'];
const WEEK = ['Воскресенье', 'Понедельник', 'Вторник', 'Среда', 'Четверг', 'Пятница', 'Суббота'];
export const SIGN: Record<string, string> = { RUB: '₽', KZT: '₸', USD: '$', EUR: '€' };

function group(n: number): string {
  return String(n).replace(/\B(?=(\d{3})+(?!\d))/g, NB);
}

/** «66 358,90» или «700», если копеек нет. */
export function fmt(kop: number): string {
  const a = Math.abs(Math.round(kop));
  const int = Math.floor(a / 100);
  const fr = a % 100;
  return (kop < 0 ? '−' : '') + group(int) + (fr ? ',' + String(fr).padStart(2, '0') : '');
}

/** Всегда с копейками, как в 1С: «122 000,00». */
export function fmt2(kop: number): string {
  const a = Math.abs(Math.round(kop));
  return (kop < 0 ? '−' : '') + group(Math.floor(a / 100)) + ',' + String(a % 100).padStart(2, '0');
}

/** Для вставки в 1С: «66358,90». */
export function plain(kop: number): string {
  const a = Math.abs(Math.round(kop));
  return String(Math.floor(a / 100)) + ',' + String(a % 100).padStart(2, '0');
}

export function rub(kop: number, cur = 'RUB'): string {
  return fmt(kop) + NB + (SIGN[cur] || cur);
}

/** Разбор набранной суммы: «1 500,50», «1500.5». */
export function parseAmount(text: string): number | null {
  const t = text.replace(/[\s ]/g, '').replace(',', '.');
  if (!/^\d*(\.\d{0,2})?$/.test(t) || t === '' || t === '.') return null;
  return Math.round(parseFloat(t) * 100);
}

/** Сумма для поля ввода: 600000 → «6000», 150050 → «1500,50». */
export function amountText(kop: number): string {
  const a = Math.abs(Math.round(kop));
  const fr = a % 100;
  return String(Math.floor(a / 100)) + (fr ? ',' + String(fr).padStart(2, '0') : '');
}

function parts(iso: string): [number, number, number] {
  const p = iso.split('-').map(Number);
  return [p[0], p[1], p[2]];
}

export function toDate(iso: string): Date {
  const [y, m, d] = parts(iso);
  return new Date(Date.UTC(y, m - 1, d));
}

export function todayIso(): string {
  const d = new Date();
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
}

export function addDays(iso: string, n: number): string {
  const d = toDate(iso);
  d.setUTCDate(d.getUTCDate() + n);
  return d.toISOString().slice(0, 10);
}

export function diffDays(a: string, b: string): number {
  return Math.round((toDate(b).getTime() - toDate(a).getTime()) / 86400000);
}

/** Курс валюты: «81,23», «0,1735». */
export function rateText(r: number): string {
  const digits = r >= 10 ? 2 : r >= 1 ? 3 : 4;
  return r.toFixed(digits).replace(/\.?0+$/, '').replace('.', ',');
}

/** «31 октября» */
export function dMon(iso: string): string {
  const [, m, d] = parts(iso);
  return d + NB + GEN[m - 1];
}

/** «31.10.2026» */
export function dNum(iso: string): string {
  const [y, m, d] = parts(iso);
  return `${String(d).padStart(2, '0')}.${String(m).padStart(2, '0')}.${y}`;
}

/** «21–31 октября» или «28 сентября – 9 октября» */
export function dRange(a: string, b: string): string {
  const [, ma, da] = parts(a);
  const [, mb, db] = parts(b);
  if (ma === mb) return `${da}–${db}${NB}${GEN[mb - 1]}`;
  return `${dMon(a)} – ${dMon(b)}`;
}

export function weekdayLabel(iso: string): string {
  return WEEK[toDate(iso).getUTCDay()] + ', ' + dMon(iso);
}

export function monthName(iso: string): string {
  return NOM[parts(iso)[1] - 1];
}

export function plural(n: number, one: string, few: string, many: string): string {
  const m = n % 100;
  const l = n % 10;
  if (m > 10 && m < 20) return many;
  if (l === 1) return one;
  if (l >= 2 && l <= 4) return few;
  return many;
}

export function daysText(n: number): string {
  return n + NB + plural(n, 'день', 'дня', 'дней');
}
