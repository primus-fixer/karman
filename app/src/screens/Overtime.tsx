import { Fragment, useEffect, useRef, useState, type KeyboardEvent } from 'react';
import { api, LOCKED, message, type AiDay, type AiRule, type Attachment, type OrderScan, type OtExported, type OtHours, type OtMonth, type OtPay, type OtRow, type OtRules, type OtSheet, type Snapshot, type Trip, type TripView, type WorkDay, type WorkOrder } from '../api';
import { costText } from './Ai';
import { aiOn, useAi } from '../aiState';
import { NB, addDays, amountText, dMon, dNum, dRange, monthName, parseAmount, plural, rub, toDate, todayIso } from '../format';
import { ORDER_PHOTO, attachFile, pickFiles } from '../media';
import { Cut, ErrorText, Icon, Seg, Thumbs } from '../ui';

type Run = (p: Promise<Snapshot>, ok?: string) => Promise<Snapshot | null>;

export type OtActions = {
  run: Run;
  flash: (text: string, undo?: () => void, long?: boolean) => void;
  locked: () => void;
  view: (a: Attachment) => void;
  /** Телефон: наряд снимается камерой, а отчёт собирается на компьютере. */
  mobile: boolean;
};

// ——— Часы, время и имена файлов ———

/** Минуты → «2», «1,5», «0,75» — как в таблице. */
export function hoursText(min: number): string {
  if (min % 60 === 0) return String(min / 60);
  return (min / 60).toFixed(2).replace(/0+$/, '').replace('.', ',');
}

/** «1 час», «16 часов», «97,5 часа». */
export function hoursFull(min: number): string {
  return hoursText(min) + NB + (min % 60 ? 'часа' : plural(min / 60, 'час', 'часа', 'часов'));
}

/** Минуты от полуночи → «17:00». */
const clock = (min: number) => `${Math.floor(min / 60) % 24}:${String(min % 60).padStart(2, '0')}`;

/** Набранное время → «8:30». Понимает «8», «830», «8.30», «8,30», «24:00». Пусто — '', ошибка — null. */
export function timeInput(s: string): string | null {
  const t = s.trim().replace(/[.,юбЮБ\-\s]+/g, ':');
  if (!t) return '';
  const m = /^(\d{1,2}):(\d{2})$/.exec(t) || /^(\d{1,2})()$/.exec(t) || /^(\d{1,2})(\d{2})$/.exec(t);
  if (!m) return null;
  let h = Number(m[1]);
  const min = Number(m[2] || 0);
  if (h === 24 && min === 0) h = 0;
  if (h > 23 || min > 59) return null;
  return `${h}:${String(min).padStart(2, '0')}`;
}

const WD = ['Вс', 'Пн', 'Вт', 'Ср', 'Чт', 'Пт', 'Сб'];

/** «Пт, 23.10» */
function dayLabel(iso: string): string {
  return `${WD[toDate(iso).getUTCDay()]}, ${dNum(iso).slice(0, 5)}`;
}

/** Метка дня: выходной, праздник или рабочая суббота по переносу. */
function dayKind(row: OtRow): { label: string; cls: string } | null {
  const wd = toDate(row.date).getUTCDay();
  const weekend = wd === 0 || wd === 6;
  if (row.off) return { label: weekend ? 'выходной' : 'праздник', cls: 'pos' };
  return weekend ? { label: 'рабочий', cls: '' } : null;
}

/** Часы дня одной строкой: «2 ч ×1,5 · 7 ч ×2». */
function hoursShort(h: OtHours | null): string {
  if (!h) return '';
  const parts: string[] = [];
  if (h.x15) parts.push(`${hoursText(h.x15)}${NB}ч${NB}×1,5`);
  if (h.x2 + h.off) parts.push(`${hoursText(h.x2 + h.off)}${NB}ч${NB}×2`);
  return parts.join(' · ') || 'без переработки';
}

/** Как имя файла в отчёте (ядро: zip::safe_name). */
function safeName(s: string): string {
  const t = s
    .replace(/[\\/:*?"<>|\u0000-\u001f]/g, ' ')
    .split(/\s+/)
    .filter(Boolean)
    .join(' ');
  return t.slice(0, 80) || 'файл';
}

/** «Наряд 3-2-23 с 23.10.2026 по 26.10.2026» — как назовётся файл в папке «Наряды». */
export function orderFileName(o: { number: string; from: string; to: string }): string {
  const num = o.number.trim().replace(/[/\\]/g, '-');
  const [a, b] = o.from <= o.to ? [o.from, o.to] : [o.to, o.from];
  const dates = a === b ? `за ${dNum(a)}` : `с ${dNum(a)} по ${dNum(b)}`;
  return safeName(num ? `Наряд ${num} ${dates}` : `Наряд ${dates}`);
}

/** Имя папки отчёта: «Переработки Иванов И. И. октябрь 2026». */
function folderName(snap: Snapshot, what: string): string {
  const who = (snap.settings['ot.name'] || '').trim();
  return safeName(who ? `Переработки ${who} ${what}` : `Переработки ${what}`);
}

const ordersText = (n: number) => `${n}${NB}${plural(n, 'наряд', 'наряда', 'нарядов')}`;

/** «23–26 октября» или «30 октября». */
const orderDates = (o: WorkOrder) => (o.from === o.to ? dMon(o.from) : o.from < o.to ? dRange(o.from, o.to) : dRange(o.to, o.from));

/** «октябрь 2026» из «2026-10». */
const monthTitle = (m: string) => `${monthName(m + '-01').toLowerCase()} ${m.slice(0, 4)}`;

/** Месяц, на который приходится больше всего дней поездки. */
function mainMonth(t: Trip): string {
  const [a, b] = t.start <= t.end ? [t.start, t.end] : [t.end, t.start];
  const count = new Map<string, number>();
  for (let d = a, i = 0; d <= b && i < 400; d = addDays(d, 1), i++) count.set(d.slice(0, 7), (count.get(d.slice(0, 7)) ?? 0) + 1);
  let best = a.slice(0, 7);
  for (const [m, n] of count) if (n > (count.get(best) ?? 0)) best = m;
  return best;
}

const lunchText = (m: number) => (m === 60 ? '1 час' : m % 60 === 0 ? `${m / 60} часа` : m > 60 ? `${hoursText(m)} часа` : `${m} минут`);
const stepText = (m: number) => (m === 60 ? 'часа' : m === 30 ? 'получаса' : `${m} минут`);

/** Как считаются часы — одним абзацем. */
export function rulesText(r: OtRules): string {
  const lunch = r.lunch ? `, без обеда ${lunchText(r.lunch)}, если работа захватила время с 12 до 13` : '';
  const round = r.step > 1 ? `Часы округляются до ${stepText(r.step)}` : 'Часы не округляются';
  return (
    `Будни: с ${clock(r.day_end)} до ${clock(r.day_end + r.first)} — коэффициент 1,5, позже — 2. ` +
    `Выходные и праздники — вся смена с коэффициентом 2${lunch}. ${round}. ` +
    'Смена за полночь считается днём начала. Выходные, праздники и переносы — по производственному календарю.'
  );
}

/** Правила из настроек — как в ядре (overtime::Rules::from_settings). */
function rulesFrom(s: Record<string, string>): OtRules {
  const r: OtRules = { day_start: 8 * 60, day_end: 17 * 60, first: 120, lunch: 60, step: 30 };
  const start = timeInput(s['ot.day_start'] || '');
  if (start) {
    const [h, m] = start.split(':').map(Number);
    if (h * 60 + m >= 5 * 60 && h * 60 + m <= 12 * 60) r.day_start = h * 60 + m;
  }
  const end = timeInput(s['ot.day_end'] || '');
  if (end) {
    const [h, m] = end.split(':').map(Number);
    if (h * 60 + m >= 12 * 60 && h * 60 + m <= 22 * 60) r.day_end = h * 60 + m;
  }
  const lunch = Number(s['ot.lunch']);
  if (s['ot.lunch'] && Number.isInteger(lunch) && lunch >= 0 && lunch <= 120) r.lunch = lunch;
  const step = Number(s['ot.round']);
  if ([1, 15, 30, 60].includes(step)) r.step = step;
  return r;
}

// ——— Итоги и сумма к оплате ———

function Totals({ sheet, rules, title }: { sheet: OtSheet; rules: OtRules; title: string }) {
  const afterTwo = clock(rules.day_end + rules.first);
  const parts = [sheet.x2 ? `будни после ${afterTwo} — ${hoursText(sheet.x2)}` : '', sheet.off ? `выходные — ${hoursText(sheet.off)}` : ''].filter(Boolean);
  return (
    <>
      <h2 className="sec">{title}</h2>
      <div className="figs">
        <div className="fig">
          <span className="lab">Коэффициент 1,5</span>
          <span className="val">{hoursFull(sheet.x15)}</span>
          <span className="small muted">
            будни {clock(rules.day_end)}–{afterTwo}
          </span>
        </div>
        <div className="fig">
          <span className="lab">Коэффициент 2</span>
          <span className="val">{hoursFull(sheet.x2 + sheet.off)}</span>
          <span className="small muted">{parts.length ? parts.join(', ') : `будни после ${afterTwo} и выходные`}</span>
        </div>
      </div>
    </>
  );
}

/** Оклад и премия — для суммы к оплате. */
export function SalaryForm({ snap, run, onDone }: { snap: Snapshot; run: Run; onDone?: () => void }) {
  const cur = (k: string) => {
    const v = Number(snap.settings[k] || 0);
    return v > 0 ? amountText(v) : '';
  };
  const [salary, setSalary] = useState(() => cur('ot.salary'));
  const [bonus, setBonus] = useState(() => cur('ot.bonus'));
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const save = async () => {
    const s = salary.trim() ? parseAmount(salary) : 0;
    const b = bonus.trim() ? parseAmount(bonus) : 0;
    if (s == null || b == null || s > 1e10 || b > 1e10) return setError('Суммы — в рублях, например 90 000');
    setError(null);
    setBusy(true);
    const first = await run(api.settingSet('ot.salary', String(s)));
    const done = first && (await run(api.settingSet('ot.bonus', String(b)), s + b ? 'Оклад и премия сохранены' : 'Оклад и премия убраны'));
    setBusy(false);
    if (done) onDone?.();
  };
  const enter = (e: KeyboardEvent) => e.key === 'Enter' && save();
  return (
    <div className="stack gap10">
      <div className="form-grid" style={{ gridTemplateColumns: 'repeat(2, minmax(0, 1fr))' }}>
        <label className="field">
          <span>Оклад, ₽</span>
          <input className="input num" inputMode="decimal" value={salary} onChange={(e) => setSalary(e.target.value)} onKeyDown={enter} />
        </label>
        <label className="field">
          <span>Премия, ₽</span>
          <input className="input num" inputMode="decimal" value={bonus} onChange={(e) => setBonus(e.target.value)} onKeyDown={enter} />
        </label>
      </div>
      <ErrorText text={error} />
      <div className="sec-actions">
        <button type="button" className="btn line small" disabled={busy} onClick={save}>
          Сохранить
        </button>
        {onDone && (
          <button type="button" className="link-btn" style={{ color: 'var(--ink2)' }} onClick={onDone}>
            Отмена
          </button>
        )}
      </div>
    </div>
  );
}

function NormForm({ p, run, onDone }: { p: OtPay; run: Run; onDone: () => void }) {
  const key = 'ot.norm.' + p.month;
  const [text, setText] = useState(hoursText(p.norm));
  const [error, setError] = useState<string | null>(null);
  const save = async () => {
    const h = Number(text.trim().replace(',', '.'));
    if (!(h > 0 && h < 400)) return setError('Норма — часы за месяц, например 176');
    if (await run(api.settingSet(key, String(h)), `Норма за ${monthTitle(p.month)}: ${hoursFull(Math.round(h * 60))}`)) onDone();
  };
  const reset = async () => {
    if (await run(api.settingSet(key, ''), 'Норма — по производственному календарю')) onDone();
  };
  return (
    <div className="stack gap6">
      <div className="sec-actions" style={{ alignItems: 'center' }}>
        <input
          className="input num"
          inputMode="decimal"
          aria-label={`Норма часов за ${monthTitle(p.month)}`}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && save()}
          style={{ width: 96, textAlign: 'right' }}
        />
        <span className="small muted">ч</span>
        <button type="button" className="btn line small" onClick={save}>
          Сохранить
        </button>
        {p.custom && (
          <button type="button" className="link-btn" onClick={reset}>
            По календарю
          </button>
        )}
      </div>
      <ErrorText text={error} />
    </div>
  );
}

function PayLine({ snap, p, run }: { snap: Snapshot; p: OtPay; run: Run }) {
  const [open, setOpen] = useState(false);
  const [edit, setEdit] = useState<'norm' | 'salary' | null>(null);
  const base = (Number(snap.settings['ot.salary']) || 0) + (Number(snap.settings['ot.bonus']) || 0);
  return (
    <div className="stack gap6">
      <div className="result">
        <span>К оплате за {monthTitle(p.month)}</span>
        <span>{rub(p.sum)}</span>
      </div>
      <span className="small muted">До вычета налога. Сумма видна только здесь, в отчёт не входит.</span>
      <button type="button" className="link-btn" style={{ alignSelf: 'flex-start' }} aria-expanded={open} onClick={() => setOpen(!open)}>
        {open ? 'Скрыть расчёт' : 'Как посчитано'}
      </button>
      {open && (
        <div className="stack gap0">
          <div className="kv">
            <span>Часы</span>
            <span>
              {hoursText(p.x15)} × 1,5 + {hoursText(p.x2)} × 2
            </span>
          </div>
          <div className="kv">
            <span>Оклад и премия</span>
            <span>{rub(base)}</span>
          </div>
          <div className="kv">
            <span>
              Норма за {monthName(p.month + '-01').toLowerCase()}
              <span className="small muted" style={{ display: 'block' }}>
                {p.custom ? 'своя' : p.exact ? 'по производственному календарю' : 'оценка: календаря на этот год нет — проверьте'}
              </span>
            </span>
            <span>{hoursFull(p.norm)}</span>
          </div>
          <div className="kv">
            <span>Час работы: оклад и премия / норма</span>
            <span>{rub(p.rate)}</span>
          </div>
          <div className="sec-actions" style={{ marginTop: 6 }}>
            <button type="button" className="btn line small" aria-pressed={edit === 'norm'} onClick={() => setEdit(edit === 'norm' ? null : 'norm')}>
              Поправить норму
            </button>
            <button type="button" className="btn line small" aria-pressed={edit === 'salary'} onClick={() => setEdit(edit === 'salary' ? null : 'salary')}>
              Изменить оклад
            </button>
          </div>
          {edit === 'norm' && <NormForm p={p} run={run} onDone={() => setEdit(null)} />}
          {edit === 'salary' && <SalaryForm snap={snap} run={run} onDone={() => setEdit(null)} />}
        </div>
      )}
    </div>
  );
}

/** Сумма к оплате по месяцам; без оклада — предложение его указать. */
function Pay({ snap, pay, sheet, run }: { snap: Snapshot; pay: OtPay[]; sheet: OtSheet; run: Run }) {
  const [ask, setAsk] = useState(false);
  const hasHours = sheet.x15 + sheet.x2 + sheet.off > 0;
  const base = (Number(snap.settings['ot.salary']) || 0) + (Number(snap.settings['ot.bonus']) || 0);
  if (pay.length) return pay.map((p) => <PayLine key={p.month} snap={snap} p={p} run={run} />);
  if (!hasHours || base > 0) return null;
  return (
    <div className="stack gap10" style={{ paddingTop: 10, borderTop: '1px solid var(--line)' }}>
      <span className="small muted">Укажите оклад и премию — покажу, сколько начислят за переработки. Сумма видна только здесь, в отчёт не входит.</span>
      {ask ? (
        <SalaryForm snap={snap} run={run} onDone={() => setAsk(false)} />
      ) : (
        <button type="button" className="btn line small" style={{ alignSelf: 'flex-start' }} onClick={() => setAsk(true)}>
          Указать оклад и премию
        </button>
      )}
    </div>
  );
}

// ——— Отчёт: таблица Excel и папка «Наряды» ———

/** Фамилия в имени папки отчёта. */
export function NameField({ snap, run }: { snap: Snapshot; run: Run }) {
  const saved = snap.settings['ot.name'] || '';
  const [text, setText] = useState(saved);
  useEffect(() => setText(saved), [saved]);
  const commit = () => {
    const v = text.trim().replace(/\s+/g, ' ');
    if (v !== saved) run(api.settingSet('ot.name', v), v ? 'Фамилия в имени отчёта сохранена' : 'Фамилия из имени отчёта убрана');
  };
  return (
    <label className="setrow">
      <span>
        <span>Фамилия в имени отчёта</span>
        <span className="small muted">{saved ? `Например, «${folderName(snap, monthTitle(todayIso().slice(0, 7)))}»` : 'Добавится в имя папки отчёта'}</span>
      </span>
      <input
        className="input"
        aria-label="Фамилия в имени отчёта"
        value={text}
        maxLength={60}
        placeholder="Иванов И. И."
        onChange={(e) => setText(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => e.key === 'Enter' && (e.target as HTMLInputElement).blur()}
        style={{ flex: 'none', width: 180, height: 40 }}
      />
    </label>
  );
}

function Report({ snap, act, what, folder, orders, names }: { snap: Snapshot; act: OtActions; what: { trip: string } | { month: string }; folder: string; orders: WorkOrder[]; names: string[] }) {
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState<OtExported | null>(null);
  if (act.mobile) {
    return <p className="small muted">Отчёт — таблица Excel и папка с нарядами — собирается на компьютере: свяжите устройства по Wi-Fi и выгрузите там.</p>;
  }
  const withPhoto = orders.filter((o) => o.att.length).length;
  const noPhoto = orders.map((o, i) => (o.att.length ? '' : names[i] || orderFileName(o))).filter(Boolean);
  const go = async () => {
    setBusy(true);
    try {
      const r = await api.overtimeExport(what);
      setDone(r);
      act.flash('Отчёт сохранён: ' + r.path, undefined, true);
    } catch (e) {
      if (message(e) === LOCKED) act.locked();
      else act.flash(message(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="stack gap10" style={{ paddingTop: 12, borderTop: '1px solid var(--line)' }}>
      <button type="button" className="btn work" style={{ alignSelf: 'flex-start', display: 'flex', alignItems: 'center', gap: 8 }} disabled={busy} onClick={go}>
        <Icon name="download" size={18} stroke={2} />
        {busy ? 'Собираю отчёт…' : 'Отчёт в «Загрузки»'}
      </button>
      <p className="small muted">
        Папка «{folder}»: таблица Excel{withPhoto ? ` и ${ordersText(withPhoto)} в PDF в папке «Наряды»` : ''}.
      </p>
      {noPhoto.length > 0 && <p className="small warn-text">Без снимка, в папку не попадут: {noPhoto.join('; ')}.</p>}
      {!snap.settings['ot.name'] && <NameField snap={snap} run={act.run} />}
      {done && (
        <p className="small muted pos" role="status">
          Сохранено: {done.path}
          {done.notes.length ? `. Не вошло: ${done.notes.join('; ')}` : ''}
        </p>
      )}
    </div>
  );
}

// ——— Наряды ———

/** Распознавание нарядов на этом устройстве: ready — ключ есть и функция включена. */
type AiState = { ready: boolean; key: boolean };

function ScanButton({ busy, small, onClick }: { busy: boolean; small?: boolean; onClick: () => void }) {
  return (
    <button type="button" className={'btn line' + (small ? ' small' : '')} style={{ display: 'flex', alignItems: 'center', gap: 6 }} disabled={busy} onClick={onClick}>
      <Icon name="spark" size={small ? 16 : 18} />
      {busy ? 'Claude читает наряд…' : 'Распознать'}
    </button>
  );
}

const SCAN_NOTE = 'Обычно до минуты. Листы наряда уходят в Claude; в «Карман» попадёт только то, что вы проверите и сохраните.';

function OrderForm({ initial, trip, act, ai, onScanned, onDone }: { initial: WorkOrder; trip: Trip; act: OtActions; ai: boolean; onScanned: (base: WorkOrder, scan: OrderScan) => void; onDone: () => void }) {
  const [o, setO] = useState(initial);
  const [busy, setBusy] = useState(false);
  const [scanning, setScanning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const set = (p: Partial<WorkOrder>) => setO((x) => ({ ...x, ...p }));
  const add = async (camera: boolean) => {
    const files = await pickFiles({ accept: camera ? 'image/*' : 'image/*,application/pdf', capture: camera, multiple: !camera });
    if (!files.length) return;
    setBusy(true);
    setError(null);
    try {
      const added: Attachment[] = [];
      for (const f of files) added.push(await attachFile(f, ORDER_PHOTO));
      setO((x) => ({ ...x, att: [...x.att, ...added] }));
    } catch (e) {
      if (message(e) === LOCKED) act.locked();
      else setError(message(e));
    } finally {
      setBusy(false);
    }
  };
  const scan = async () => {
    setScanning(true);
    setError(null);
    try {
      onScanned(o, await api.aiOrderScan(trip.id, o.att.map((a) => a.id)));
    } catch (e) {
      if (message(e) === LOCKED) act.locked();
      else setError(message(e));
    } finally {
      setScanning(false);
    }
  };
  const save = async () => {
    if (!o.from || !o.to) return setError('Укажите, с какого и по какое число выдан наряд');
    if (o.to < o.from) return setError('Дата «по» раньше даты «с»');
    setError(null);
    setBusy(true);
    const s = await act.run(api.orderSave({ ...o, number: o.number.trim(), object: o.object.trim() }), initial.id ? 'Наряд сохранён' : 'Наряд добавлен');
    setBusy(false);
    if (s) onDone();
  };
  const outside = o.from && o.to && (o.to < trip.start || o.from > trip.end);
  return (
    <div className="card stack gap12" role="group" aria-label={initial.id ? 'Наряд' : 'Новый наряд'}>
      <div className="form-grid">
        <label className="field">
          <span>Номер наряда</span>
          <input className="input" value={o.number} maxLength={40} placeholder="например, 3-2/23" autoFocus={!act.mobile} onChange={(e) => set({ number: e.target.value })} />
        </label>
        <label className="field">
          <span>С</span>
          <input className="input" type="date" value={o.from} onChange={(e) => set({ from: e.target.value })} />
        </label>
        <label className="field">
          <span>По</span>
          <input className="input" type="date" value={o.to} onChange={(e) => set({ to: e.target.value })} />
        </label>
      </div>
      <label className="field">
        <span>Объект и работы</span>
        <input className="input" value={o.object} maxLength={300} placeholder="например, Чекмагуш. 2СШ яч. №12–15" onChange={(e) => set({ object: e.target.value })} />
      </label>
      <div className="stack gap6">
        <span className="small muted">Снимки или PDF наряда — все листы{ai ? '. Claude может прочитать их и заполнить наряд и дни' : ''}</span>
        {o.att.length > 0 && <Thumbs items={o.att} onOpen={act.view} onRemove={(id) => set({ att: o.att.filter((a) => a.id !== id) })} />}
        <div className="sec-actions">
          {act.mobile && (
            <button type="button" className="btn line" style={{ display: 'flex', alignItems: 'center', gap: 6 }} disabled={busy || scanning} onClick={() => add(true)}>
              <Icon name="camera" size={18} />
              Снять
            </button>
          )}
          <button type="button" className="btn line" style={{ display: 'flex', alignItems: 'center', gap: 6 }} disabled={busy || scanning} onClick={() => add(false)}>
            <Icon name="file" size={18} />
            {act.mobile ? 'Из файлов' : 'Снимок или PDF'}
          </button>
          {ai && o.att.length > 0 && <ScanButton busy={scanning} onClick={scan} />}
        </div>
        {scanning && <p className="small muted">{SCAN_NOTE}</p>}
      </div>
      <p className="small muted">В отчёте: «{orderFileName(o)}.pdf»</p>
      {outside && <p className="small warn-text">Даты наряда — вне поездки ({dRange(trip.start, trip.end)}).</p>}
      <ErrorText text={error} />
      <div className="sec-actions">
        <button type="button" className="btn work" disabled={busy || scanning} onClick={save}>
          {initial.id ? 'Сохранить' : 'Добавить наряд'}
        </button>
        <button type="button" className="btn line" onClick={onDone}>
          Отмена
        </button>
      </div>
    </div>
  );
}

function OrderItem({ o, name, act, ai, onEdit, onScanned }: { o: WorkOrder; name: string; act: OtActions; ai: boolean; onEdit: () => void; onScanned: (base: WorkOrder, scan: OrderScan) => void }) {
  const [busy, setBusy] = useState(false);
  const [scanning, setScanning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const remove = async () => {
    const s = await act.run(api.orderDelete(o.id));
    if (s) act.flash(o.number ? `Наряд ${o.number} удалён` : 'Наряд удалён', () => act.run(api.orderSave(o), 'Наряд возвращён'));
  };
  /** Ещё снимок — сразу в наряд, без формы. */
  const attach = async () => {
    const files = await pickFiles({ accept: act.mobile ? 'image/*' : 'image/*,application/pdf', capture: act.mobile, multiple: !act.mobile });
    if (!files.length) return;
    setBusy(true);
    try {
      const added: Attachment[] = [];
      for (const f of files) added.push(await attachFile(f, ORDER_PHOTO));
      await act.run(api.orderSave({ ...o, att: [...o.att, ...added] }), added.length > 1 ? 'Снимки добавлены' : 'Снимок добавлен');
    } catch (e) {
      if (message(e) === LOCKED) act.locked();
      else act.flash(message(e));
    } finally {
      setBusy(false);
    }
  };
  const scan = async () => {
    setScanning(true);
    setError(null);
    try {
      onScanned(o, await api.aiOrderScan(o.trip, o.att.map((a) => a.id)));
    } catch (e) {
      if (message(e) === LOCKED) act.locked();
      else setError(message(e));
    } finally {
      setScanning(false);
    }
  };
  return (
    <div className="otorder">
      <div className="between">
        <span style={{ fontWeight: 500 }}>{o.number ? `Наряд № ${o.number}` : 'Наряд без номера'}</span>
        <span className="small muted" style={{ flex: 'none' }}>
          {orderDates(o)}
        </span>
      </div>
      {o.object && <span>{o.object}</span>}
      {o.att.length ? <Thumbs items={o.att} onOpen={act.view} /> : <span className="small warn-text">Нет снимка — в папку отчёта не попадёт</span>}
      <span className="small muted muted3">В отчёте: «{name}.pdf»</span>
      <div className="sec-actions">
        <button type="button" className="btn line small" style={{ display: 'flex', alignItems: 'center', gap: 6 }} disabled={busy || scanning} onClick={attach}>
          <Icon name="camera" size={16} />
          {o.att.length ? 'Ещё снимок' : 'Снимок'}
        </button>
        {ai && o.att.length > 0 && <ScanButton small busy={scanning} onClick={scan} />}
        <button type="button" className="btn line small" disabled={scanning} onClick={onEdit}>
          Изменить
        </button>
        <button type="button" className="link-btn warn-text" disabled={scanning} onClick={remove}>
          Удалить
        </button>
      </div>
      {scanning && <p className="small muted">{SCAN_NOTE}</p>}
      <ErrorText text={error} />
    </div>
  );
}

const RULE_TAG: Record<AiRule, string> = {
  weekday: 'будни',
  off: 'выходной',
  evening: 'допуск вечером',
  before: 'до ввода в бригаду',
  after: 'после вывода',
  outside: 'вне поездки',
  no_end: 'нет окончания',
};

const shortDate = (iso: string) => dNum(iso).slice(0, 5);

/** Кто вы в наряде — одной строкой. */
function meText(scan: OrderScan): { text: string; warn: boolean } {
  const me = scan.me;
  if (!me.known) return { text: 'ФИО не указано в настройках переработок, поэтому дни не отсеяны по составу бригады. Проверьте, с какого дня вы в бригаде.', warn: true };
  if (!me.found) return { text: 'Вашей фамилии в наряде не нашлось — проверьте, ваши ли это дни.', warn: true };
  const parts: string[] = [];
  if (me.from_start) parts.push(`${me.name}: в бригаде с выдачи наряда`);
  else if (me.added) parts.push(`${me.name}: в бригаде с ${shortDate(me.added)}${me.added_time ? ', ' + me.added_time : ''} — дни до этого не берём`);
  if (me.removed) parts.push(`выведены из бригады ${shortDate(me.removed)}${me.removed_time ? ', ' + me.removed_time : ''} — дни после не берём`);
  return { text: parts.join('; '), warn: false };
}

type ReviewDay = AiDay & { on: boolean; s: string; e: string; was: OtRow | null; same: boolean };

/** Наряд, прочитанный Claude: поля, состав бригады и дни — всё правится до сохранения. */
function ScanReview({ base, scan, tv, act, onDone }: { base: WorkOrder; scan: OrderScan; tv: TripView; act: OtActions; onDone: () => void }) {
  const t = tv.trip;
  const rules = tv.ot.rules;
  const [number, setNumber] = useState(scan.number || base.number);
  const [object, setObject] = useState(scan.object || base.object);
  const [from, setFrom] = useState(scan.from || base.from);
  const [to, setTo] = useState(scan.to || base.to);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [days, setDays] = useState<ReviewDay[]>(() =>
    scan.days.map((d) => {
      const was = tv.ot.sheet.rows.find((r) => r.date === d.date && (r.start || r.comment)) ?? null;
      const same = !!was && was.start === d.start && was.end === d.end;
      // Своё время, уже вписанное в день, без спроса не заменяется: такие дни — без галочки.
      return { ...d, on: d.take && !same && !was?.start, s: d.start, e: d.end, was, same };
    }),
  );
  const upd = (i: number, p: Partial<ReviewDay>) => setDays((list) => list.map((d, j) => (j === i ? { ...d, ...p } : d)));
  const me = meText(scan);
  const n = days.filter((d) => d.on).length;
  const save = async () => {
    if (!from || !to) return setError('Укажите, с какого и по какое число выдан наряд');
    if (to < from) return setError('Дата «по» раньше даты «с»');
    const chosen = days.filter((d) => d.on);
    const out: WorkDay[] = [];
    for (const d of chosen) {
      const s = timeInput(d.s);
      const e = timeInput(d.e);
      if (!s || !e) return setError(`${dayLabel(d.date)}: время работы — например 8:00 и 23:50`);
      if (s === e) return setError(`${dayLabel(d.date)}: начало и окончание совпадают`);
      out.push({ id: '', trip: t.id, date: d.date, start: s, end: e, comment: d.was?.comment || object.trim() });
    }
    const before: WorkDay[] = chosen.map((d) => ({ id: '', trip: t.id, date: d.date, start: d.was?.start || '', end: d.was?.end || '', comment: d.was?.comment || '' }));
    const prev = base.id ? (tv.ot.orders.find((o) => o.id === base.id) ?? null) : null;
    const order: WorkOrder = { ...base, number: number.trim(), object: object.trim(), from, to };
    setError(null);
    setBusy(true);
    let id = '';
    const s = await act.run(
      api.otApply([order], [], out).then((a) => {
        id = a.orders[0] ?? '';
        return a.snap;
      }),
    );
    setBusy(false);
    if (!s) return;
    onDone();
    const what = (order.number ? `наряд ${order.number}` : 'наряд') + (out.length ? ` и ${out.length}${NB}${plural(out.length, 'день', 'дня', 'дней')}` : '');
    act.flash(`Записано: ${what}`, () => act.run(api.otApply(prev ? [prev] : [], prev ? [] : [id], before).then((a) => a.snap), 'Возвращено'));
  };
  return (
    <div className="card stack gap12" role="group" aria-label="Наряд от Claude">
      <div className="between" style={{ alignItems: 'center' }}>
        <span style={{ fontWeight: 500, display: 'flex', alignItems: 'center', gap: 6 }}>
          <Icon name="spark" size={18} />
          Claude прочитал наряд — проверьте
        </span>
        {scan.usd > 0 && <span className="small muted muted3">{costText(scan.usd)}</span>}
      </div>
      <div className="form-grid">
        <label className="field">
          <span>Номер наряда</span>
          <input className="input" value={number} maxLength={40} onChange={(e) => setNumber(e.target.value)} />
        </label>
        <label className="field">
          <span>С</span>
          <input className="input" type="date" value={from} onChange={(e) => setFrom(e.target.value)} />
        </label>
        <label className="field">
          <span>По</span>
          <input className="input" type="date" value={to} onChange={(e) => setTo(e.target.value)} />
        </label>
      </div>
      <label className="field">
        <span>Объект и работы</span>
        <input className="input" value={object} maxLength={300} onChange={(e) => setObject(e.target.value)} />
      </label>
      <div className="stack gap6">
        <p className={'small muted' + (me.warn ? ' warn-text' : '')}>{me.text}</p>
        {scan.brigade && <p className="small muted muted3">Бригада: {scan.brigade}</p>}
        {scan.problems && <p className="small warn-text">Claude не уверен: {scan.problems}</p>}
      </div>
      <div className="stack gap0">
        <span style={{ marginBottom: 4 }}>Дни по наряду</span>
        {days.length === 0 && <p className="small warn-text">Строк ежедневного допуска Claude не нашёл — дни впишите сами в таблице «По дням».</p>}
        {days.map((d, i) => (
          <div key={d.date} className={'scanday' + (d.on ? '' : ' muted3')}>
            <label className="scanpick">
              <input type="checkbox" checked={d.on} aria-label={`Взять ${dayLabel(d.date)}`} onChange={(e) => upd(i, { on: e.target.checked })} />
              <span>{dayLabel(d.date)}</span>
              <span className={'tag ' + (d.take ? '' : 'warn')}>{RULE_TAG[d.rule]}</span>
            </label>
            <span className="small muted num scanorder">
              по наряду {d.order_start}–{d.order_end || '?'}
            </span>
            <span className="scantimes">
              <input className="input num" inputMode="decimal" aria-label={`${dayLabel(d.date)}: начало`} placeholder="8:00" value={d.s} disabled={!d.on} onChange={(e) => upd(i, { s: e.target.value })} onBlur={() => upd(i, { s: timeInput(d.s) ?? d.s })} />
              <span>–</span>
              <input className="input num" inputMode="decimal" aria-label={`${dayLabel(d.date)}: окончание`} placeholder="17:00" value={d.e} disabled={!d.on} onChange={(e) => upd(i, { e: e.target.value })} onBlur={() => upd(i, { e: timeInput(d.e) ?? d.e })} />
            </span>
            {d.was && <span className="small muted">{d.same ? 'уже так записано' : d.was.start ? `сейчас ${d.was.start}–${d.was.end}` : 'сейчас без времени'}</span>}
          </div>
        ))}
      </div>
      <p className="small muted">
        Будни — с {clock(rules.day_start)}, окончание по наряду, но не раньше {clock(rules.day_end)}. Выходные, праздники и допуск после {clock(rules.day_end)} — по наряду. Дни до ввода в бригаду не берутся. Комментарий дня — «Объект и работы», если свой не вписан.
      </p>
      <ErrorText text={error} />
      <div className="sec-actions">
        <button type="button" className="btn work" disabled={busy} onClick={save}>
          {n ? `Сохранить наряд и ${n}${NB}${plural(n, 'день', 'дня', 'дней')}` : 'Сохранить наряд'}
        </button>
        <button type="button" className="btn line" disabled={busy} onClick={onDone}>
          Отмена
        </button>
      </div>
    </div>
  );
}

function Orders({ tv, act }: { tv: TripView; act: OtActions }) {
  const t = tv.trip;
  const orders = tv.ot.orders;
  const [form, setForm] = useState<WorkOrder | null>(null);
  const [review, setReview] = useState<{ base: WorkOrder; scan: OrderScan } | null>(null);
  const status = useAi();
  const ai: AiState = { ready: aiOn(status, 'orders'), key: status ? status.key : true };
  const scanned = (base: WorkOrder, scan: OrderScan) => {
    setForm(null);
    setReview({ base, scan });
  };
  const blank = (): WorkOrder => {
    const last = orders[orders.length - 1];
    const from = last && last.to >= t.start && last.to <= t.end ? last.to : t.start;
    return { id: '', trip: t.id, number: '', object: '', from, to: t.end >= from ? t.end : from, att: [], note: '' };
  };
  const reviewOf = (base: WorkOrder, scan: OrderScan) => <ScanReview key={'scan' + base.id} base={base} scan={scan} tv={tv} act={act} onDone={() => setReview(null)} />;
  return (
    <section className="stack gap0">
      <div className="between" style={{ alignItems: 'center', marginBottom: 6 }}>
        <h2 className="sec">Наряды</h2>
        {!form && !review && (
          <button type="button" className="btn line small" style={{ display: 'flex', alignItems: 'center', gap: 6 }} onClick={() => setForm(blank())}>
            <Icon name="plus" size={16} stroke={2} />
            Добавить наряд
          </button>
        )}
      </div>
      {review && !review.base.id && reviewOf(review.base, review.scan)}
      {form && !form.id && <OrderForm initial={form} trip={t} act={act} ai={ai.ready} onScanned={scanned} onDone={() => setForm(null)} />}
      {orders.length === 0 && !form && !review && <p className="row muted">Нарядов пока нет. Снимок или PDF наряда ляжет в отчёт отдельным файлом — с номером и датами в имени.</p>}
      {orders.map((o, i) =>
        review && review.base.id === o.id ? (
          reviewOf(review.base, review.scan)
        ) : form && form.id === o.id ? (
          <OrderForm key={o.id} initial={form} trip={t} act={act} ai={ai.ready} onScanned={scanned} onDone={() => setForm(null)} />
        ) : (
          <OrderItem key={o.id} o={o} name={tv.ot.names[i] || orderFileName(o)} act={act} ai={ai.ready && !review} onEdit={() => setForm(o)} onScanned={scanned} />
        ),
      )}
      {!ai.key && <p className="small muted muted3">Наряд по снимкам может заполнить Claude — ключ API вводится в настройках, раздел «ИИ».</p>}
    </section>
  );
}

// ——— Таблица по дням ———

function HeadRow({ rules }: { rules: OtRules }) {
  const a = clock(rules.day_end);
  const b = clock(rules.day_end + rules.first);
  return (
    <div className="otday head" aria-hidden="true">
      <span>Дата</span>
      <span>Время работы</span>
      <span className="oth">
        {a}–{b}
        <br />
        ×1,5
      </span>
      <span className="oth">
        после {b}
        <br />
        ×2
      </span>
      <span className="oth">
        выходные
        <br />
        ×2
      </span>
      <span>Комментарий</span>
    </div>
  );
}

function RowCells({ row, showTrip }: { row: OtRow; showTrip?: boolean }) {
  const kind = dayKind(row);
  const h = row.hours;
  const num = (v: number) => (h ? (v ? hoursText(v) : '–') : '');
  return (
    <>
      <span className="otd">
        <span>{dayLabel(row.date)}</span>
        {kind && <span className={'tag ' + kind.cls}>{kind.label}</span>}
        {showTrip && row.trip && <span className="tag">{row.trip}</span>}
      </span>
      <span className="ott num">{row.start ? `${row.start} – ${row.end}` : <span className="muted3">—</span>}</span>
      <span className="oth">{num(h?.x15 ?? 0)}</span>
      <span className="oth">{num(h?.x2 ?? 0)}</span>
      <span className="oth">{num(h?.off ?? 0)}</span>
      <span className="otc">{row.comment}</span>
      <span className="oths small muted">{hoursShort(h)}</span>
    </>
  );
}

function TotalRow({ sheet }: { sheet: OtSheet }) {
  return (
    <div className="otday total">
      <span className="otd">Итого</span>
      <span className="ott" />
      <span className="oth">{hoursText(sheet.x15)}</span>
      <span className="oth">{hoursText(sheet.x2)}</span>
      <span className="oth">{hoursText(sheet.off)}</span>
      <span className="otc" />
      <span className="oths">
        {hoursText(sheet.x15)}
        {NB}ч{NB}×1,5 · {hoursText(sheet.x2 + sheet.off)}
        {NB}ч{NB}×2
      </span>
    </div>
  );
}

type DayValue = { start: string; end: string; comment: string };

function DayEditor({
  row,
  prev,
  orders,
  last,
  wide,
  onSave,
  onClear,
  onCancel,
}: {
  row: OtRow;
  prev: OtRow | null;
  orders: WorkOrder[];
  last: boolean;
  wide: boolean;
  onSave: (v: DayValue, next: boolean) => Promise<boolean>;
  onClear: () => void;
  onCancel: () => void;
}) {
  const [start, setStart] = useState(row.start);
  const [end, setEnd] = useState(row.end);
  const [comment, setComment] = useState(row.comment);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const first = useRef<HTMLInputElement>(null);
  useEffect(() => {
    first.current?.focus();
    first.current?.select();
  }, []);
  const submit = async (next: boolean) => {
    const s = timeInput(start);
    const e = timeInput(end);
    if (s === null) return setError('Начало — время, например 8:00');
    if (e === null) return setError('Окончание — время, например 23:00');
    if (!s !== !e) return setError(s ? 'Во сколько закончили работу?' : 'Во сколько начали работу?');
    if (s && s === e) return setError('Начало и окончание совпадают');
    setError(null);
    setBusy(true);
    const ok = await onSave({ start: s, end: e, comment: comment.trim() }, next);
    if (!ok) setBusy(false);
  };
  const keys = (e: KeyboardEvent) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      submit(!last);
    } else if (e.key === 'Escape') onCancel();
  };
  const fromOrders = orders.filter((o) => o.object && o.object !== comment.trim());
  const copyPrev = prev && (prev.start !== start || prev.end !== end || (!comment.trim() && prev.comment));
  return (
    <div className="oted" role="group" aria-label={`${dayLabel(row.date)}: время работы`} onKeyDown={keys}>
      <div className="ottimes">
        <label className="field">
          <span>Начало</span>
          <input ref={first} className="input num" inputMode="decimal" placeholder="8:00" value={start} onChange={(e) => setStart(e.target.value)} onBlur={() => setStart((v) => timeInput(v) ?? v)} />
        </label>
        <label className="field">
          <span>Окончание</span>
          <input className="input num" inputMode="decimal" placeholder="17:00" value={end} onChange={(e) => setEnd(e.target.value)} onBlur={() => setEnd((v) => timeInput(v) ?? v)} />
        </label>
        <label className="field otcm">
          <span>Комментарий: где и что делали</span>
          <input className="input" value={comment} maxLength={500} onChange={(e) => setComment(e.target.value)} />
        </label>
      </div>
      {(copyPrev || fromOrders.length > 0) && (
        <div className="chips">
          {prev && copyPrev && (
            <button
              type="button"
              className="chip"
              onClick={() => {
                setStart(prev.start);
                setEnd(prev.end);
                if (!comment.trim() && prev.comment) setComment(prev.comment);
              }}
            >
              Как {dNum(prev.date).slice(0, 5)}: {prev.start}–{prev.end}
            </button>
          )}
          {fromOrders.map((o) => (
            <button key={o.id} type="button" className="chip" title={o.object} onClick={() => setComment(o.object)}>
              Из наряда{o.number ? ' ' + o.number : ''}
            </button>
          ))}
        </div>
      )}
      <ErrorText text={error} />
      <div className="sec-actions">
        <button type="button" className="btn work" disabled={busy} onClick={() => submit(false)}>
          Сохранить
        </button>
        {!last && (
          <button type="button" className="btn line" disabled={busy} onClick={() => submit(true)}>
            Сохранить и дальше
          </button>
        )}
        {(row.start || row.comment) && (
          <button type="button" className="link-btn warn-text" disabled={busy} onClick={onClear}>
            Очистить день
          </button>
        )}
        <button type="button" className="btn line" onClick={onCancel}>
          Отмена
        </button>
      </div>
      {wide && <p className="small muted">Enter — сохранить и перейти к следующему дню, Esc — отмена. Работали за полночь — пишите окончание как есть, например 2:00.</p>}
    </div>
  );
}

function Days({ tv, act, wide }: { tv: TripView; act: OtActions; wide: boolean }) {
  const t = tv.trip;
  const { sheet, rules, orders } = tv.ot;
  const rows = sheet.rows;
  const [open, setOpen] = useState<string | null>(null);
  const save = async (row: OtRow, v: DayValue, next: boolean) => {
    const day: WorkDay = { id: '', trip: t.id, date: row.date, ...v };
    const s = await act.run(api.daySave(day));
    if (!s) return false;
    const i = rows.findIndex((r) => r.date === row.date);
    setOpen(next && i >= 0 && i + 1 < rows.length ? rows[i + 1].date : null);
    return true;
  };
  const clear = async (row: OtRow) => {
    const before: WorkDay = { id: '', trip: t.id, date: row.date, start: row.start, end: row.end, comment: row.comment };
    const s = await act.run(api.daySave({ ...before, start: '', end: '', comment: '' }));
    if (s) {
      setOpen(null);
      act.flash(`${dNum(row.date)} очищен`, () => act.run(api.daySave(before), 'День возвращён'));
    }
  };
  const prevWithTimes = (i: number) => {
    for (let j = i - 1; j >= 0; j--) if (rows[j].start) return rows[j];
    return null;
  };
  return (
    <section className="stack gap0">
      <h2 className="sec" style={{ marginBottom: 6 }}>
        По дням
      </h2>
      <p className="small muted" style={{ marginBottom: 8 }}>
        Нажмите на день и впишите, во сколько начали и закончили работу. Часы переработки посчитаются сами.
      </p>
      <div className="otdays">
        <HeadRow rules={rules} />
        {rows.map((r, i) => (
          <Fragment key={r.date}>
            <button type="button" className={'otday' + (r.off ? ' off' : '') + (open === r.date ? ' open' : '')} aria-expanded={open === r.date} onClick={() => setOpen(open === r.date ? null : r.date)}>
              <RowCells row={r} />
            </button>
            {open === r.date && (
              <DayEditor
                key={r.date}
                row={r}
                prev={prevWithTimes(i)}
                orders={orders.filter((o) => (o.from <= o.to ? o.from <= r.date && r.date <= o.to : o.to <= r.date && r.date <= o.from))}
                last={i === rows.length - 1}
                wide={wide}
                onSave={(v, next) => save(r, v, next)}
                onClear={() => clear(r)}
                onCancel={() => setOpen(null)}
              />
            )}
          </Fragment>
        ))}
        <TotalRow sheet={sheet} />
      </div>
    </section>
  );
}

// ——— Общая таблица за месяц ———

function MonthPart({ snap, tv, act }: { snap: Snapshot; tv: TripView; act: OtActions }) {
  const [month, setMonth] = useState(() => mainMonth(tv.trip));
  const [data, setData] = useState<OtMonth | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    api.overtimeMonth(month).then(
      (d) => {
        if (!alive) return;
        setData(d);
        setError(null);
      },
      (e) => {
        if (!alive) return;
        if (message(e) === LOCKED) act.locked();
        else setError(message(e));
      },
    );
    return () => {
      alive = false;
    };
    // Новые записи меняют сводку — таблица пересчитывается.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [month, snap]);
  const months = Array.from(new Set([month, ...(data?.months ?? [])])).sort().reverse();
  const shown = data && data.month === month ? data : null;
  return (
    <>
      <label className="field">
        <span>Месяц</span>
        <select className="input" value={month} onChange={(e) => setMonth(e.target.value)}>
          {months.map((m) => (
            <option key={m} value={m}>
              {monthTitle(m)[0].toUpperCase() + monthTitle(m).slice(1)}
            </option>
          ))}
        </select>
      </label>
      <ErrorText text={error} />
      {!shown && !error && <p className="small muted">Считаю…</p>}
      {shown && shown.sheet.rows.length === 0 && <p className="row muted">В этом месяце поездок нет.</p>}
      {shown && shown.sheet.rows.length > 0 && (
        <>
          <section className="card stack gap10">
            <Totals sheet={shown.sheet} rules={shown.rules} title={`Переработки за ${shown.title}`} />
            <Pay snap={snap} pay={shown.pay} sheet={shown.sheet} run={act.run} />
            <Report snap={snap} act={act} what={{ month }} folder={folderName(snap, shown.title)} orders={shown.orders} names={shown.names} />
          </section>
          <section className="stack gap0">
            <h2 className="sec" style={{ marginBottom: 6 }}>
              Наряды месяца
            </h2>
            {shown.orders.length === 0 && <p className="row muted">Нарядов за этот месяц нет.</p>}
            {shown.orders.map((o, i) => (
              <div key={o.id} className="otorder">
                <div className="between">
                  <span>«{shown.names[i] || orderFileName(o)}.pdf»</span>
                  <span className="tag" style={{ flex: 'none' }}>
                    {o.trip}
                  </span>
                </div>
                {!o.att.length && <span className="small warn-text">Нет снимка — в папку отчёта не попадёт</span>}
              </div>
            ))}
          </section>
          <section className="stack gap0">
            <h2 className="sec" style={{ marginBottom: 6 }}>
              По дням
            </h2>
            <p className="small muted" style={{ marginBottom: 8 }}>
              Из всех поездок месяца, с днями между ними. Время и комментарии правятся в карточке поездки.
            </p>
            <div className="otdays">
              <HeadRow rules={shown.rules} />
              {shown.sheet.rows.map((r) => (
                <div key={r.date} className={'otday' + (r.off ? ' off' : '')}>
                  <RowCells row={r} showTrip />
                </div>
              ))}
              <TotalRow sheet={shown.sheet} />
            </div>
          </section>
        </>
      )}
    </>
  );
}

// ——— Раздел поездки ———

export default function OvertimeTab({ snap, tv, act, wide }: { snap: Snapshot; tv: TripView; act: OtActions; wide: boolean }) {
  const [view, setView] = useState<'trip' | 'month'>('trip');
  const ot = tv.ot;
  return (
    <div className="stack gap20">
      <Seg cols
        label="Переработки"
        value={view}
        onChange={setView}
        options={[
          ['trip', 'Эта поездка'],
          ['month', 'Общая за месяц'],
        ]}
      />
      {view === 'trip' ? (
        <>
          <section className="card stack gap10">
            <Totals sheet={ot.sheet} rules={ot.rules} title="Переработки по поездке" />
            <Pay snap={snap} pay={ot.pay} sheet={ot.sheet} run={act.run} />
            <Report snap={snap} act={act} what={{ trip: tv.trip.id }} folder={folderName(snap, `${tv.trip.id} ${tv.trip.city}`)} orders={ot.orders} names={ot.names} />
          </section>
          <Orders tv={tv} act={act} />
          <Days tv={tv} act={act} wide={wide} />
        </>
      ) : (
        <MonthPart snap={snap} tv={tv} act={act} />
      )}
      <p className="small muted">{rulesText(ot.rules)} Правила и оклад — в настройках.</p>
    </div>
  );
}

// ——— Настройки ———

export function OvertimeSettings({ snap, run }: { snap: Snapshot; run: Run }) {
  const s = snap.settings;
  const rules = rulesFrom(s);
  const endSaved = clock(rules.day_end).padStart(5, '0');
  const [end, setEnd] = useState(endSaved);
  useEffect(() => setEnd(endSaved), [endSaved]);
  const startSaved = clock(rules.day_start).padStart(5, '0');
  const [start, setStart] = useState(startSaved);
  useEffect(() => setStart(startSaved), [startSaved]);
  // Поле времени присылает значение, только когда время введено целиком.
  const endBad = /^\d\d:\d\d$/.test(end) && (end < '12:00' || end > '22:00');
  const startBad = /^\d\d:\d\d$/.test(start) && (start < '05:00' || start > '12:00');
  return (
    <Cut id="ot" title="Переработки">
      <p className="small muted" style={{ marginBottom: 8 }}>
        Наряды и часы — в карточке поездки, раздел «Наряды». Здесь — как считать часы и оклад для суммы к оплате.
      </p>
      <NameField snap={snap} run={run} />
      <label className="setrow">
        <span>
          <span>Начало рабочего дня</span>
          <span className={'small muted' + (startBad ? ' warn-text' : '')}>{startBad ? 'Время — от 5:00 до 12:00' : 'С него в будни пишется работа, когда наряд распознаёт Claude'}</span>
        </span>
        <input
          className="input num"
          type="time"
          aria-label="Начало рабочего дня"
          value={start}
          min="05:00"
          max="12:00"
          onChange={(e) => {
            const v = e.target.value;
            setStart(v);
            const t = timeInput(v);
            if (t && /^\d\d:\d\d$/.test(v) && v >= '05:00' && v <= '12:00' && v !== startSaved) run(api.settingSet('ot.day_start', t), 'Начало рабочего дня — ' + t);
          }}
          style={{ flex: 'none', width: 112, height: 40, fontSize: 16 }}
        />
      </label>
      <label className="setrow">
        <span>
          <span>Конец рабочего дня</span>
          <span className={'small muted' + (endBad ? ' warn-text' : '')}>{endBad ? 'Время — от 12:00 до 22:00' : 'С него в будни идут переработки: два часа с коэффициентом 1,5, дальше — 2'}</span>
        </span>
        <input
          className="input num"
          type="time"
          aria-label="Конец рабочего дня"
          value={end}
          min="12:00"
          max="22:00"
          onChange={(e) => {
            const v = e.target.value;
            setEnd(v);
            const t = timeInput(v);
            if (t && /^\d\d:\d\d$/.test(v) && v >= '12:00' && v <= '22:00' && v !== endSaved) run(api.settingSet('ot.day_end', t), 'Конец рабочего дня — ' + t);
          }}
          style={{ flex: 'none', width: 112, height: 40, fontSize: 16 }}
        />
      </label>
      <div className="stack gap6" style={{ padding: '10px 0', borderTop: '1px solid var(--line)' }}>
        <span>Обед в выходной</span>
        <span className="small muted">Не считается, если работа захватила время с 12 до 13</span>
        <Seg cols small
          label="Обед в выходной"
          value={String(rules.lunch)}
          onChange={(v) => run(api.settingSet('ot.lunch', v), v === '0' ? 'Обед в выходной не вычитается' : 'Обед в выходной — ' + lunchText(Number(v)))}
          options={[
            ['0', 'Нет'],
            ['30', '30 мин'],
            ['60', '1 час'],
            ['90', '1,5 часа'],
          ]}
        />
      </div>
      <div className="stack gap6" style={{ padding: '10px 0', borderTop: '1px solid var(--line)' }}>
        <span>Округление часов</span>
        <Seg cols small
          label="Округление часов"
          value={String(rules.step)}
          onChange={(v) => run(api.settingSet('ot.round', v), v === '1' ? 'Часы не округляются' : 'Часы округляются до ' + stepText(Number(v)))}
          options={[
            ['1', 'Нет'],
            ['15', '15 мин'],
            ['30', '30 мин'],
            ['60', '1 час'],
          ]}
        />
      </div>
      <div className="stack gap6" style={{ padding: '10px 0', borderTop: '1px solid var(--line)' }}>
        <span>Оклад и премия</span>
        <span className="small muted">Для суммы к оплате: час работы — оклад и премия, делённые на норму часов месяца. Сумма видна только в карточке поездки, в отчёт не входит.</span>
        <SalaryForm key={(s['ot.salary'] || '') + '/' + (s['ot.bonus'] || '')} snap={snap} run={run} />
      </div>
      <p className="small muted" style={{ paddingTop: 10, borderTop: '1px solid var(--line)' }}>
        {rulesText(rules)}
      </p>
    </Cut>
  );
}
