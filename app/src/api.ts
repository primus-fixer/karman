import { invoke } from '@tauri-apps/api/core';
import { todayIso } from './format';

// Типы повторяют ответы ядра (karman-core, serde).

export type OpKind = 'work' | 'food' | 'personal' | 'income' | 'advance' | 'reimbursement' | 'return' | 'debt' | 'transfer';
export type DocKind = 'photo' | 'qr' | 'file';

export type Attachment = { id: string; mime: string; name: string; size: number };

export type Op = {
  id: string;
  date: string;
  kind: OpKind;
  cat: string;
  title: string;
  amount: number;
  account: string;
  trip: string | null;
  doc: DocKind | null;
  sent: boolean;
  family: boolean;
  debt: string | null;
  inflow: boolean;
  orig: number | null;
  cur: string | null;
  note: string;
  /** Снимки и файлы чеков; в ответе ядра поля нет, если вложений нет. */
  att?: Attachment[];
  /** Перевод: счёт, на который ушли деньги. */
  to?: string | null;
  /** Перевод: сколько пришло в валюте счёта-получателя, если она не рубли. */
  to_amount?: number | null;
  /** Оплата регулярного платежа: какого и за какую дату. */
  plan?: string | null;
  plan_date?: string | null;
  /** Перевод на цель или с неё. */
  goal?: string | null;
};

/** Условия вклада. */
export type Deposit = { rate: number; opened: string; until: string | null; capitalize: boolean };
/** Прогноз по вкладу: проценты за месяц, до конца срока и сумма к концу срока. */
export type DepositView = { account: string; month: number; income: number; at_end: number; until: string | null; days_left: number | null };
export type Goal = { id: string; title: string; target: number; due: string | null; account: string; done: boolean; note: string };
export type GoalView = { goal: Goal; saved: number; left: number; months_left: number | null; per_month: number | null };
export type Budget = { id: 'personal' | 'family'; total: number | null; cats: Record<string, number> };
export type CatView = { cat: string; limit: number | null; spent: number };
export type BudgetView = { id: 'personal' | 'family'; start: string; end: string; total: number | null; spent: number; cats: CatView[]; days_left: number };

export type StatSpan = 'month' | 'quarter' | 'year';
export type CatStat = { cat: string; amount: number; prev: number };
export type MonthStat = { month: string; income: number; spent: number; saved: number };
export type TripStat = { id: string; city: string; start: string; end: string; days: number; per_diem: number; left: number; debt: number; confirmed: number; finished: boolean };
export type TripsYear = { year: number; count: number; days: number; per_diem: number; left: number; debt: number; confirmed: number; items: TripStat[] };
/** Статистика периода; пока он идёт (`partial`), сравнение — с теми же днями прошлого. */
export type Stats = {
  from: string;
  to: string;
  prev_from: string;
  prev_to: string;
  partial: boolean;
  spent: number;
  prev_spent: number;
  income: number;
  cats: CatStat[];
  months: MonthStat[];
  trips: TripsYear;
};

export type DebtDir = 'i_owe' | 'owed_to_me';
export type Debt = { id: string; who: string; dir: DebtDir; amount: number; date: string; due: string | null; note: string; closed: boolean };
export type DebtView = { debt: Debt; repaid: number; left: number; overdue: boolean; plan: string | null; next: string | null };
export type Every = 'month' | 'week' | 'year';
export type Plan = {
  id: string;
  title: string;
  /** Сумма в валюте счёта. */
  amount: number;
  account: string;
  cat: string;
  every: Every;
  start: string;
  until: string | null;
  /** Автосписание: записывается само. */
  auto: boolean;
  /** За сколько дней напоминать; null — как в настройках. */
  remind: number | null;
  /** Кредит: платёж уменьшает этот долг. */
  debt: string | null;
  skipped?: string[];
  paused: boolean;
  note: string;
};
export type PlanView = { plan: Plan; next: string | null; overdue: number; paid: number; amount_rub: number };
/** Ближайший срок: платёж по графику или возврат долга. */
export type Due = { kind: 'plan' | 'debt' | 'deposit'; id: string; title: string; date: string; amount: number; days: number; lead: number; soon: boolean; auto: boolean; i_owe: boolean };
/** Напоминания: платформа, разрешение на уведомления, режим на компьютере. */
export type RemindStatus = { platform: 'android' | 'windows' | 'other'; permission: 'granted' | 'denied' | 'prompt'; desktop: 'open' | 'windows' };

export const STATUSES = ['request', 'advance_received', 'in_trip', 'awaiting_report', 'review', 'closed'] as const;
export type TripStatus = (typeof STATUSES)[number];

export type Trip = {
  id: string;
  city: string;
  start: string;
  end: string;
  plan_start: string;
  plan_end: string;
  status: TripStatus;
  plan: number[];
  fixed_rate: number | null;
  own_rate: number | null;
  req_no: string;
  req_date: string | null;
  goal: string;
  region: string;
  plan_notes: string[];
  notes: string[];
  deadline: string | null;
  just: string | null;
};

export type Settlement = {
  days: number;
  plan_days: number;
  elapsed: number;
  finished: boolean;
  rate: number;
  per_diem: number;
  per_diem_full: number;
  issued: number;
  reimbursed: number;
  returned: number;
  confirmed: number;
  no_doc: number;
  no_doc_count: number;
  food: number;
  tagged: number;
  balance: number;
  leftover: number;
  debt: number;
  debt_due: number;
  lost: number;
  days_left: number;
};

export type ReportRow = { code: string; name: string; plan: number; fact: number; over: number };

export type TripView = {
  trip: Trip;
  rate: number;
  rate_source: 'own' | 'fixed' | 'settings';
  s: Settlement;
  rows: ReportRow[];
  over_total: number;
  auto_just: string;
};

export type Account = { id: string; name: string; short: string; currency: string; opening: number; kind: string; order: number; archived: boolean; deposit?: Deposit | null };
export type AccountView = { account: Account; balance: number; balance_rub: number };

export type Snapshot = {
  today: string;
  device: string;
  rate: number;
  /** Курсы для пересчёта: ЦБ, если включены и загружены, иначе из настроек. */
  fx: Record<string, number>;
  /** Курс ЦБ на дату операции (настройка fx.auto, по умолчанию да). */
  fx_auto: boolean;
  /** Дата загруженных курсов ЦБ. */
  fx_date: string | null;
  /** Курсы из настроек — запасные. */
  fx_manual: Record<string, number>;
  accounts: AccountView[];
  total: number;
  held: number;
  due: number;
  owed: number;
  debt_due: number;
  mine: number;
  trips: TripView[];
  ops: Op[];
  settings: Record<string, string>;
  debts: DebtView[];
  plans: PlanView[];
  /** Платежи и сроки долгов на месяц вперёд и всё просроченное. */
  upcoming: Due[];
  /** Долги отдельно от «Моих денег». */
  i_owe: number;
  owed_to_me: number;
  /** За сколько дней напоминать. */
  remind_days: number;
  /** На накопительных счетах и вкладах — строка «Отложено». */
  saved: number;
  /** Отложенное входит в «Мои деньги» (настройка, по умолчанию нет). */
  saved_in_mine: boolean;
  deposits: DepositView[];
  goals: GoalView[];
  /** Бюджет на месяц: личный и семейный. */
  budgets: BudgetView[];
  /** С какого числа начинается месяц бюджета. */
  budget_day: number;
};

export type Check = { ok: boolean; text: string };
export type DeviceInfo = { id: string; name: string; os: string; arch: string; version: string; data_dir: string; encrypted: boolean; cipher: string; events: number; unknown: number };
export type PairingView = { uri: string; qr_svg: string; hosts: string[]; port: number };
export type SyncStatus = { running: boolean; log: Check[] };
export type SyncResult = { peer_name: string; sent: number; received: number; files_sent: number; files_received: number; hint: string };
/** Курс валюты на дату: cbr — ЦБ, manual — из настроек, none — курса нет; fallback — сайт ЦБ не ответил, это последние сохранённые курсы. */
export type FxQuote = { rate: number; source: 'cbr' | 'manual' | 'none'; date: string | null; fallback?: boolean };
export type ReceiptView = { date: string; time: string; sum: number; fn_number: string; fd: string; fp: string; kind: string };
export type LockStatus = {
  pin: boolean;
  locked: boolean;
  bio: boolean;
  bio_ready: boolean;
  bio_kind: '' | 'fingerprint' | 'face' | 'hello';
  recover: '' | 'device' | 'hello';
  wait: number;
  left: number;
  recovery: boolean;
  key: string;
  error: string | null;
  notice: string | null;
};

/** Ответ команд с данными, пока «Карман» заблокирован. */
export const LOCKED = 'locked';

/** Приложение открыто внутри Tauri, а не в обычном браузере. */
export const inApp = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

const today = () => todayIso();

export const api = {
  /** tz — смещение местного времени от UTC в минутах: для напоминаний в нужный час. */
  snapshot: () => invoke<Snapshot>('snapshot', { today: today(), tz: -new Date().getTimezoneOffset() }),
  opSave: (op: Op) => invoke<Snapshot>('op_save', { today: today(), op }),
  opDelete: (id: string) => invoke<Snapshot>('op_delete', { today: today(), id }),
  tripSave: (trip: Trip) => invoke<Snapshot>('trip_save', { today: today(), trip }),
  tripDelete: (id: string) => invoke<Snapshot>('trip_delete', { today: today(), id }),
  tripAdvance: (id: string) => invoke<Snapshot>('trip_advance', { today: today(), id }),
  tripSettle: (id: string) => invoke<Snapshot>('trip_settle', { today: today(), id }),
  accountSave: (account: Account) => invoke<Snapshot>('account_save', { today: today(), account }),
  settingSet: (key: string, value: string) => invoke<Snapshot>('setting_set', { today: today(), key, value }),
  deviceInfo: () => invoke<DeviceInfo>('device_info'),
  dbCheck: () => invoke<Check[]>('db_check'),
  calcCheck: () => invoke<Check[]>('calc_check'),
  syncStart: () => invoke<PairingView>('sync_start'),
  syncStatus: () => invoke<SyncStatus>('sync_status'),
  syncStop: () => invoke<void>('sync_stop'),
  syncConnect: (uri: string) => invoke<SyncResult>('sync_connect', { uri }),
  receiptParse: (text: string) => invoke<ReceiptView>('receipt_parse', { text }),
  blobPut: (mime: string, data: string) => invoke<{ id: string; size: number }>('blob_put', { mime, data }),
  blobUrl: (id: string) => invoke<string | null>('blob_url', { id }),
  receiptsZip: (trip: string) => invoke<string>('receipts_zip', { trip }),
  fxRate: (cur: string, date: string) => invoke<FxQuote>('fx_rate', { today: today(), cur, date }),
  debtSave: (debt: Debt, account: string | null) => invoke<Snapshot>('debt_save', { today: today(), debt, money: account ? { account } : null }),
  debtDelete: (id: string) => invoke<Snapshot>('debt_delete', { today: today(), id }),
  debtPay: (id: string, amount: number, account: string, date: string) => invoke<Snapshot>('debt_pay', { today: today(), id, amount, account, date }),
  planSave: (plan: Plan) => invoke<Snapshot>('plan_save', { today: today(), plan }),
  planDelete: (id: string) => invoke<Snapshot>('plan_delete', { today: today(), id }),
  planPay: (id: string, date: string, amount: number | null, account: string | null) => invoke<Snapshot>('plan_pay', { today: today(), id, date, amount, account }),
  planSkip: (id: string, date: string) => invoke<Snapshot>('plan_skip', { today: today(), id, date }),
  goalSave: (goal: Goal) => invoke<Snapshot>('goal_save', { today: today(), goal }),
  goalDelete: (id: string) => invoke<Snapshot>('goal_delete', { today: today(), id }),
  /** «Отложить» на цель или, с take, «Взять из цели». */
  goalMove: (id: string, amount: number, account: string, date: string, take: boolean) => invoke<Snapshot>('goal_move', { today: today(), id, amount, account, date, take }),
  budgetSave: (budget: Budget) => invoke<Snapshot>('budget_save', { today: today(), budget }),
  stats: (span: StatSpan, anchor: string) => invoke<Stats>('stats', { today: today(), span, anchor }),
  exportXlsx: (span: StatSpan, anchor: string) => invoke<string>('export_xlsx', { today: today(), span, anchor }),
  remindStatus: (ask: boolean) => invoke<RemindStatus>('remind_status', { ask }),
  remindDesktop: (mode: 'open' | 'windows') => invoke<string>('remind_desktop', { mode }),
  lockStatus: () => invoke<LockStatus>('lock_status'),
  pinUnlock: (code: string) => invoke<LockStatus>('pin_unlock', { code }),
  pinSet: (code: string, current: string | null) => invoke<LockStatus>('pin_set', { code, current }),
  pinOff: (current: string | null) => invoke<LockStatus>('pin_off', { current }),
  bioUnlock: () => invoke<LockStatus>('bio_unlock'),
  bioSet: (on: boolean) => invoke<LockStatus>('bio_set', { on }),
  pinRecover: () => invoke<LockStatus>('pin_recover'),
  lockNow: () => invoke<LockStatus>('lock_now'),
  appVisible: (visible: boolean) => invoke<LockStatus>('app_visible', { visible }),
};

/** Ядро сообщает, что «Карман» заблокировался (5 минут в фоне). */
export async function onLocked(cb: () => void): Promise<() => void> {
  const { listen } = await import('@tauri-apps/api/event');
  return listen('locked', cb);
}

/** Загрузились свежие курсы ЦБ — остатки на счетах в валюте пересчитаны. */
export async function onFx(cb: () => void): Promise<() => void> {
  const { listen } = await import('@tauri-apps/api/event');
  return listen('fx', cb);
}

/** Сканирование QR-кода камерой телефона. */
export async function scanQr(): Promise<string> {
  const scanner = await import('@tauri-apps/plugin-barcode-scanner');
  let perm = await scanner.checkPermissions();
  if (perm !== 'granted') perm = await scanner.requestPermissions();
  if (perm !== 'granted') throw new Error('Нет доступа к камере. Разрешите его в настройках телефона');
  const res = await scanner.scan({ windowed: false, formats: [scanner.Format.QRCode] });
  return res.content;
}

export function message(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  return String(e);
}
