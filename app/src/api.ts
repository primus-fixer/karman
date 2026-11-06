import { invoke } from '@tauri-apps/api/core';

export type Check = { ok: boolean; text: string };

export type DeviceInfo = {
  id: string;
  name: string;
  os: string;
  arch: string;
  data_dir: string;
  encrypted: boolean;
  cipher: string;
  events: number;
  ops: number;
};

export type DbCheck = { rows: number; write_ms: number; read_ms: number; checks: Check[] };

export type TripRow = { name: string; status: string; headline: string; per_diem: string; fact: string; over_note: string };
export type CalcCheck = { rate: string; trips: TripRow[]; checks: Check[] };

export type OpView = { id: string; date: string; title: string; amount: string };
export type PairingView = { uri: string; qr_svg: string; hosts: string[]; port: number };
export type SyncStatus = { running: boolean; log: Check[] };
export type SyncResult = { peer_name: string; sent: number; received: number; ops: OpView[] };
export type ReceiptView = { date: string; time: string; sum: string; fn_number: string; fd: string; fp: string; kind: string };

/** Приложение открыто внутри Tauri, а не в обычном браузере. */
export const inApp = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

export const api = {
  deviceInfo: () => invoke<DeviceInfo>('device_info'),
  dbCheck: () => invoke<DbCheck>('db_check'),
  calcCheck: () => invoke<CalcCheck>('calc_check'),
  opsList: () => invoke<OpView[]>('ops_list'),
  opAddTest: (amount: string) => invoke<OpView[]>('op_add_test', { amount }),
  opDelete: (id: string) => invoke<OpView[]>('op_delete', { id }),
  syncStart: () => invoke<PairingView>('sync_start'),
  syncStatus: () => invoke<SyncStatus>('sync_status'),
  syncStop: () => invoke<void>('sync_stop'),
  syncConnect: (uri: string) => invoke<SyncResult>('sync_connect', { uri }),
  receiptParse: (text: string) => invoke<ReceiptView>('receipt_parse', { text }),
};

/** Сканирование QR-кода камерой телефона. На компьютере не используется. */
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
