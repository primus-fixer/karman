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

/** Наряд поездки: номер, объект, на какие дни выдан, снимки или PDF. */
export type WorkOrder = { id: string; trip: string; number: string; object: string; from: string; to: string; att: Attachment[]; note: string };
/** Рабочий день поездки: время «8:00» и что делали. Пустые время и комментарий — день очищается. */
export type WorkDay = { id: string; trip: string; date: string; start: string; end: string; comment: string };
/** Часы переработки за день, в минутах: будни ×1,5, будни ×2, выходные ×2. */
export type OtHours = { x15: number; x2: number; off: number };
export type OtRow = { date: string; off: boolean; start: string; end: string; comment: string; trip: string | null; hours: OtHours | null };
export type OtSheet = { rows: OtRow[]; x15: number; x2: number; off: number };
/** Сумма за переработки месяца — только для экрана. Норма и часы — в минутах. */
export type OtPay = { month: string; norm: number; exact: boolean; custom: boolean; rate: number; x15: number; x2: number; sum: number };
/** Правила подсчёта, в минутах: конец рабочего дня, сколько идёт ×1,5, обед в выходной, шаг округления. */
export type OtRules = { day_end: number; first: number; lunch: number; step: number };
/** names — имена файлов нарядов в отчёте, в том же порядке, что и наряды. */
export type TripOt = { orders: WorkOrder[]; names: string[]; sheet: OtSheet; summary: string; pay: OtPay[]; rules: OtRules };
/** Общая таблица переработок за месяц — из всех поездок месяца. */
export type OtMonth = { month: string; title: string; sheet: OtSheet; summary: string; pay: OtPay[]; orders: WorkOrder[]; names: string[]; rules: OtRules; months: string[] };
/** Отчёт по переработкам: папка в «Загрузках», сколько в ней файлов нарядов и что не вошло. */
export type OtExported = { path: string; files: number; notes: string[] };

/** Действие Claude через коннектор: что сделано и отменено ли. `at` — unix-время. */
export type ClaudeAction = { id: number; at: number; tool: string; text: string; undone: boolean };
/** Коннектор для приложения Claude: включён ли, разрешена ли запись, чем Claude запускает «Карман». */
export type ClaudeStatus = { desktop: boolean; on: boolean; write: boolean; command: string; args: string[]; dir: string; actions: ClaudeAction[] };

export type TripView = {
  trip: Trip;
  rate: number;
  rate_source: 'own' | 'fixed' | 'settings';
  s: Settlement;
  rows: ReportRow[];
  over_total: number;
  auto_just: string;
  /** Наряды и переработки поездки. */
  ot: TripOt;
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

/** Последняя копия на этом компьютере. */
export type BackupLast = { at: number; path: string; auto: boolean; bytes: number; events: number; blobs: number };
/** Резервные копии на этом устройстве: пароль, папка, как часто, последняя копия. */
export type BackupStatus = {
  desktop: boolean;
  password: boolean;
  dir: string;
  chosen: boolean;
  every: 'day' | 'week' | 'off';
  keep: number;
  last: BackupLast | null;
  error: string | null;
  running: boolean;
};
/** Что в файле копии (пароль подошёл, файл прочитан целиком). */
export type BackupInfo = { created: number; device: string; app: string; ops: number; trips: number; events: number; blobs: number; bytes: number };
export type RestoreResult = { events: number; blobs: number; ops: number; trips: number; snapshot: Snapshot };

/** Файл копии уходит в ядро кусками по 4 МБ: так он читается и на телефоне, и без лишней памяти. */
const UPLOAD_CHUNK = 4 << 20;

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
  backupStatus: () => invoke<BackupStatus>('backup_status'),
  backupPassword: (password: string) => invoke<BackupStatus>('backup_password', { password }),
  backupEvery: (every: BackupStatus['every']) => invoke<BackupStatus>('backup_every', { every }),
  /** Папка копий; `null` — папка по умолчанию. */
  backupDir: (path: string | null) => invoke<BackupStatus>('backup_dir', { path }),
  backupPickDir: () => invoke<BackupStatus>('backup_pick_dir'),
  backupNow: () => invoke<BackupLast>('backup_now'),
  /** Загружает файл копии в ядро и возвращает номер загрузки. */
  backupUpload: async (file: File, progress?: (done: number, total: number) => void): Promise<string> => {
    let id = 'new';
    for (let at = 0; at < file.size || at === 0; at += UPLOAD_CHUNK) {
      const part = new Uint8Array(await file.slice(at, at + UPLOAD_CHUNK).arrayBuffer());
      const r = await invoke<{ id: string; size: number }>('backup_upload', part, { headers: { 'x-upload': id, 'x-offset': String(at) } });
      id = r.id;
      progress?.(r.size, file.size);
      if (file.size === 0) break;
    }
    return id;
  },
  backupInspect: (id: string, password: string) => invoke<BackupInfo>('backup_inspect', { id, password }),
  backupRestore: (id: string, password: string) => invoke<RestoreResult>('backup_restore', { id, password, today: today() }),
  backupDiscard: (id: string) => invoke<null>('backup_discard', { id }),
  orderSave: (order: WorkOrder) => invoke<Snapshot>('order_save', { today: today(), order }),
  orderDelete: (id: string) => invoke<Snapshot>('order_delete', { today: today(), id }),
  daySave: (day: WorkDay) => invoke<Snapshot>('day_save', { today: today(), day }),
  /** month — «2026-10». */
  overtimeMonth: (month: string) => invoke<OtMonth>('overtime_month', { month }),
  /** Отчёт по поездке или общий за месяц: папка в «Загрузках» (на компьютере). */
  overtimeExport: (what: { trip: string } | { month: string }) => invoke<OtExported>('overtime_export', { trip: null, month: null, ...what }),
  claudeStatus: () => invoke<ClaudeStatus>('claude_status'),
  claudeSet: (on: boolean, write: boolean) => invoke<ClaudeStatus>('claude_set', { on, write }),
  claudeUndo: (id: number) => invoke<Snapshot>('claude_undo', { today: today(), id }),
  /** Файл расширения для приложения Claude — в «Загрузки». */
  claudeBundle: () => invoke<string>('claude_bundle'),
};

/** Ядро сообщает, что «Карман» заблокировался (5 минут в фоне). */
export async function onLocked(cb: () => void): Promise<() => void> {
  const { listen } = await import('@tauri-apps/api/event');
  return listen('locked', cb);
}

/** Сделана автокопия (или не получилась) — обновить сведения о копиях. */
export async function onBackup(cb: () => void): Promise<() => void> {
  const { listen } = await import('@tauri-apps/api/event');
  return listen('backup', cb);
}

/** Загрузились свежие курсы ЦБ — остатки на счетах в валюте пересчитаны. */
export async function onFx(cb: () => void): Promise<() => void> {
  const { listen } = await import('@tauri-apps/api/event');
  return listen('fx', cb);
}

/** Claude что-то записал через коннектор: обновить данные и показать, что сделано (id 0 — без отмены). */
export async function onClaude(cb: (e: { id: number; text: string }) => void): Promise<() => void> {
  const { listen } = await import('@tauri-apps/api/event');
  return listen<{ id: number; text: string }>('claude', (ev) => cb(ev.payload));
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
