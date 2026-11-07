import { invoke } from '@tauri-apps/api/core';
import { todayIso } from './format';

// Типы повторяют ответы ядра (karman-core, serde).

export type OpKind = 'work' | 'food' | 'personal' | 'income' | 'advance' | 'reimbursement' | 'return' | 'debt';
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
};

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

export type Account = { id: string; name: string; short: string; currency: string; opening: number; kind: string; order: number; archived: boolean };
export type AccountView = { account: Account; balance: number; balance_rub: number };

export type Snapshot = {
  today: string;
  device: string;
  rate: number;
  fx: Record<string, number>;
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
};

export type Check = { ok: boolean; text: string };
export type DeviceInfo = { id: string; name: string; os: string; arch: string; version: string; data_dir: string; encrypted: boolean; cipher: string; events: number };
export type PairingView = { uri: string; qr_svg: string; hosts: string[]; port: number };
export type SyncStatus = { running: boolean; log: Check[] };
export type SyncResult = { peer_name: string; sent: number; received: number; files_sent: number; files_received: number };
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
  snapshot: () => invoke<Snapshot>('snapshot', { today: today() }),
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
