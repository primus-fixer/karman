import { useEffect, useState, type CSSProperties, type FormEvent } from 'react';
import { api, message, scanQr, type Attachment, type DocKind, type Op, type Snapshot, type TripView } from './api';
import { NB, SIGN, amountText, dMon, fmt, parseAmount, plural, rub } from './format';
import { attachFile, pickFiles } from './media';
import { CATS, OWN_IN, OWN_OUT, TRIP_IN, TRIP_OUT, isIn, kindFor, statusIndex } from './logic';
import { Icon, Seg, Thumbs } from './ui';

export type EntryDraft = {
  dir: 'out' | 'in';
  amount: string;
  /** 'own' — личное, 'trip:573' — расход поездки, 'tag:573' — личное с меткой поездки. */
  scope: string;
  cat: string;
  acc: string;
  receipt: DocKind | null;
  receiptNote: string;
  /** Снимки и файлы чеков. */
  att: Attachment[];
  comment: string;
  date: string;
};

const openTrips = (snap: Snapshot) => snap.trips.filter((t) => statusIndex(t.trip.status) < 5);
const within = (tv: TripView, date: string) => date >= tv.trip.start && date <= tv.trip.end;

/** Поездка, в которой вы сейчас: в пути и сегодня в её датах. */
export function activeTrip(snap: Snapshot): TripView | null {
  const list = openTrips(snap);
  const st = (tv: TripView) => statusIndex(tv.trip.status);
  return (
    list.find((t) => st(t) === 2 && within(t, snap.today)) ||
    list.find((t) => st(t) === 2) ||
    list.find((t) => st(t) === 1 && within(t, snap.today)) ||
    null
  );
}

function accountByKind(snap: Snapshot, kind: string): string {
  const live = snap.accounts.filter((a) => !a.account.archived);
  return (live.find((a) => a.account.kind === kind) || live[0])?.account.id ?? '';
}

export function tripDraft(snap: Snapshot, tv: TripView): EntryDraft {
  return {
    ...defaultDraft(snap),
    scope: 'trip:' + tv.trip.id,
    cat: 'taxi',
    acc: accountByKind(snap, 'work'),
    date: within(tv, snap.today) ? snap.today : tv.trip.end < snap.today ? tv.trip.end : tv.trip.start,
  };
}

export function defaultDraft(snap: Snapshot): EntryDraft {
  const t = activeTrip(snap);
  return {
    dir: 'out',
    amount: '',
    scope: t ? 'trip:' + t.trip.id : 'own',
    cat: t ? 'taxi' : 'groceries',
    acc: accountByKind(snap, t ? 'work' : 'personal'),
    receipt: null,
    receiptNote: '',
    att: [],
    comment: '',
    date: snap.today,
  };
}

/** Черновик из записанной операции — чтобы её поправить. */
export function draftFromOp(o: Op): EntryDraft {
  const tripKind = o.kind === 'work' || o.kind === 'food' || o.kind === 'advance' || o.kind === 'reimbursement';
  const cat = o.kind === 'advance' ? 'advance' : o.kind === 'reimbursement' ? 'reimb' : o.cat;
  const catName = (CATS[cat] || CATS.other).n;
  return {
    dir: isIn(o) ? 'in' : 'out',
    amount: amountText(o.orig ?? o.amount),
    scope: o.trip ? (tripKind ? 'trip:' : 'tag:') + o.trip : 'own',
    cat,
    acc: o.account,
    receipt: o.doc,
    receiptNote: o.doc === 'qr' ? o.note : '',
    att: o.att ?? [],
    comment: o.title === catName ? '' : o.title,
    date: o.date,
  };
}

const isTripScope = (s: string) => s.startsWith('trip:');
const scopeTrip = (s: string) => (s === 'own' ? null : s.slice(s.indexOf(':') + 1));

function catList(dir: 'out' | 'in', scope: string): string[] {
  if (isTripScope(scope)) return dir === 'out' ? TRIP_OUT : TRIP_IN;
  return dir === 'in' ? OWN_IN : OWN_OUT;
}

const KEYS = ['1', '2', '3', '4', '5', '6', '7', '8', '9', ',', '0', 'back'];

export default function Entry({
  snap,
  initial,
  editing,
  isPhone,
  mobile,
  onSave,
  onClose,
  flash,
}: {
  snap: Snapshot;
  initial: EntryDraft;
  /** Операция, которую правим; иначе — новая. */
  editing: Op | null;
  isPhone: boolean;
  /** Есть камера для QR-кода. */
  mobile: boolean;
  onSave: (op: Op) => Promise<Snapshot | null>;
  onClose: () => void;
  flash: (text: string) => void;
}) {
  const [e, setE] = useState<EntryDraft>(initial);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [qrText, setQrText] = useState('');
  const [moreTrips, setMoreTrips] = useState(false);
  const [filesBusy, setFilesBusy] = useState(false);
  const set = (patch: Partial<EntryDraft>) => {
    setE((x) => ({ ...x, ...patch }));
    setError(null);
  };

  useEffect(() => {
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  const accounts = snap.accounts.filter((a) => !a.account.archived || a.account.id === e.acc);
  const acc = accounts.find((a) => a.account.id === e.acc) || null;
  const cur = acc ? acc.account.currency : 'RUB';
  const sign = SIGN[cur] || cur;
  const isTrip = isTripScope(e.scope);
  const tripId = scopeTrip(e.scope);
  const tv = isTrip ? snap.trips.find((t) => t.trip.id === tripId) || null : null;
  const active = activeTrip(snap);

  const pickScope = (scope: string) => {
    const list = catList(e.dir, scope);
    const patch: Partial<EntryDraft> = { scope };
    if (!list.includes(e.cat)) patch.cat = list[0];
    const kind = acc?.account.kind;
    if (!isTripScope(scope) && kind === 'work') patch.acc = accountByKind(snap, 'personal');
    if (isTripScope(scope) && kind === 'personal' && !isTripScope(e.scope)) patch.acc = accountByKind(snap, 'work');
    if (!isTripScope(scope)) patch.receipt = null;
    set(patch);
  };
  const pickDir = (dir: 'out' | 'in') => set({ dir, cat: catList(dir, e.scope)[0], receipt: dir === 'out' ? e.receipt : null });
  const pressKey = (key: string) => {
    let s = e.amount;
    if (key === 'back') s = s.slice(0, -1);
    else if (key === ',') {
      if (!s.includes(',')) s = (s || '0') + ',';
    } else {
      const i = s.indexOf(',');
      if (i >= 0 && s.length - i > 2) return;
      if (i < 0 && s.replace(/\D/g, '').length >= 8) return;
      s = s === '0' ? key : s + key;
    }
    set({ amount: s });
  };
  const onDate = (date: string) => {
    const d = date || snap.today;
    if (e.scope.startsWith('tag:')) return set({ date: d });
    const hit = openTrips(snap).find((t) => within(t, d));
    const sc = hit ? 'trip:' + hit.trip.id : 'own';
    if (sc === e.scope) return set({ date: d });
    const list = catList(e.dir, sc);
    set({ date: d, scope: sc, cat: list.includes(e.cat) ? e.cat : isTripScope(sc) && e.dir === 'out' ? 'taxi' : list[0], receipt: isTripScope(sc) ? e.receipt : null });
  };

  const applyReceipt = async (text: string) => {
    try {
      const r = await api.receiptParse(text);
      set({ receipt: 'qr', amount: amountText(r.sum), date: r.date, receiptNote: `Чек ${dMon(r.date)}, ${r.time}. ФН ${r.fn_number}, ФД ${r.fd}, ФП ${r.fp}` });
      setQrText('');
    } catch (err) {
      setError(message(err));
    }
  };
  /** Снимок с камеры (на телефоне) или файлы с компьютера. */
  const addFiles = async (camera: boolean) => {
    const files = await pickFiles({ accept: camera ? 'image/*' : 'image/*,application/pdf', capture: camera && mobile, multiple: !camera });
    if (!files.length) return;
    setFilesBusy(true);
    try {
      const added: Attachment[] = [];
      for (const f of files) added.push(await attachFile(f));
      setE((x) => ({ ...x, att: [...x.att, ...added], receipt: x.receipt === 'qr' ? 'qr' : camera ? 'photo' : x.receipt || 'file' }));
      setError(null);
    } catch (err) {
      setError(message(err));
    } finally {
      setFilesBusy(false);
    }
  };
  const removeFile = (id: string) =>
    setE((x) => {
      const att = x.att.filter((a) => a.id !== id);
      return { ...x, att, receipt: att.length || x.receipt === 'qr' ? x.receipt : null };
    });
  const pickReceipt = async (kind: DocKind) => {
    if (kind !== 'qr') return addFiles(kind === 'photo');
    if (e.receipt === 'qr') return set({ receipt: e.att.length ? 'photo' : null, receiptNote: '' });
    if (!mobile) return set({ receipt: 'qr', receiptNote: '' });
    try {
      await applyReceipt(await scanQr());
    } catch (err) {
      setError(message(err));
    }
  };

  // Подсказки под суммой и под чеком.
  const amt = parseAmount(e.amount) ?? 0;
  const fx = cur === 'RUB' ? 1 : snap.fx[cur] || 0;
  const amtRub = Math.round(amt * fx);
  let note = '';
  if (error) note = error;
  else if (e.receipt === 'qr' && e.receiptNote) note = e.receiptNote.split('.')[0];
  else if (cur !== 'RUB' && amt > 0) note = fx ? `≈${NB}${rub(amtRub)} по курсу ${String(fx).replace('.', ',')}${NB}₽ из настроек` : `Нет курса для ${cur}: укажите его в настройках`;

  const room = tv ? Math.max(0, tv.s.leftover) : 0;
  const showDocs = e.dir === 'out' && isTrip && e.cat !== 'food';
  let docNote = '';
  let docWarn = true;
  if (filesBusy) {
    docWarn = false;
    docNote = 'Сохраняю снимок…';
  } else if (e.receipt) {
    docWarn = false;
    const n = e.att.length;
    const files = n ? `Прикреплено ${n} ${plural(n, 'вложение', 'вложения', 'вложений')}. ` : '';
    docNote =
      e.receipt === 'qr'
        ? files + (e.receiptNote ? 'Чек по QR-коду: дата и сумма заполнены, расход войдёт в отчёт.' : 'Чек с QR-кодом есть, расход войдёт в отчёт.')
        : n
          ? files + 'Расход войдёт в отчёт.'
          : 'Чек есть, расход войдёт в отчёт. Снимок можно добавить позже.';
  } else if (room === 0) docNote = 'Суточные закончились: без чека сумма станет долгом перед компанией.';
  else if (amtRub > room) docNote = `Без чека ${rub(room)} — из суточных, ${rub(amtRub - room)} — долг перед компанией.`;
  else docNote = `Без чека — из суточных (осталось ${rub(room)}), сверх них — долг перед компанией.`;
  let foodNote = '';
  if (e.dir === 'out' && isTrip && e.cat === 'food')
    foodNote = 'Питание идёт из суточных, чек не нужен. ' + (room === 0 ? 'Суточные закончились: сумма станет долгом перед компанией.' : amtRub > room ? `Сверх остатка ${rub(room)} — долг перед компанией.` : `Осталось ${rub(room)}.`);

  const submitLabel = editing ? 'Сохранить изменения' : isTrip ? (e.dir === 'out' ? 'Записать в поездку ' : 'Записать доход в поездку ') + tripId : e.dir === 'out' ? 'Записать в личное' : 'Записать доход в личное';

  // Сначала текущая поездка, личное и метка; предстоящие поездки — сразу, сданные на отчёт — по кнопке.
  const tripLabel = (t: TripView) => `${t.trip.id} ${t.trip.city}`;
  const scopes: { id: string; label: string }[] = [];
  if (active) scopes.push({ id: 'trip:' + active.trip.id, label: tripLabel(active) });
  scopes.push({ id: 'own', label: 'Личное' });
  if (active) scopes.push({ id: 'tag:' + active.trip.id, label: 'Личное, метка ' + active.trip.id });
  const others = openTrips(snap).filter((t) => !active || t.trip.id !== active.trip.id);
  let hidden = 0;
  for (const t of others) {
    const id = 'trip:' + t.trip.id;
    if (statusIndex(t.trip.status) <= 2 || moreTrips || e.scope === id) scopes.push({ id, label: tripLabel(t) });
    else hidden++;
  }
  if (!scopes.some((s) => s.id === e.scope) && tripId) scopes.push({ id: e.scope, label: (e.scope.startsWith('tag:') ? 'Личное, метка ' : '') + tripId });

  const parts = e.amount.split(',');
  const amountShown = (parts[0] || '0').replace(/\B(?=(\d{3})+(?!\d))/g, NB) + (parts.length > 1 ? ',' + parts[1] : '');

  const save = async (ev?: FormEvent) => {
    ev?.preventDefault();
    if (busy) return;
    const orig = parseAmount(e.amount);
    if (!orig || orig <= 0) return setError('Введите сумму');
    if (!acc) return setError('Выберите счёт');
    if (cur !== 'RUB' && !fx) return setError(`Нет курса для ${cur}: укажите его в настройках`);
    const kind = kindFor(e.dir, isTrip, e.cat);
    const doc = kind === 'work' ? e.receipt : null;
    const op: Op = {
      id: editing ? editing.id : '',
      date: e.date || snap.today,
      kind,
      cat: e.cat,
      title: e.comment.trim() || (CATS[e.cat] || CATS.other).n,
      amount: cur === 'RUB' ? orig : Math.round(orig * fx),
      account: acc.account.id,
      trip: tripId,
      doc,
      sent: !!editing && editing.sent && !!doc,
      family: editing ? editing.family : false,
      debt: editing ? editing.debt : null,
      inflow: editing ? editing.inflow : false,
      orig: cur === 'RUB' ? null : orig,
      cur: cur === 'RUB' ? null : cur,
      note: doc === 'qr' ? e.receiptNote : editing && editing.doc !== 'qr' ? editing.note : '',
      att: e.att,
    };
    setBusy(true);
    const next = await onSave(op);
    setBusy(false);
    if (!next) return;
    let toast = `Записано: ${fmt(orig)}${NB}${sign}, ` + (isTrip ? 'поездка ' + tripId : e.scope.startsWith('tag:') ? 'личное, метка ' + tripId : 'личное');
    if (kind === 'work' && !doc) {
      const c = next.trips.find((t) => t.trip.id === tripId)?.s;
      if (c) toast = c.debt > 0 ? 'Без чека: суточных не хватает, долг перед компанией ' + rub(c.debt_due || c.debt) : 'Без чека: вычтено из суточных, осталось ' + rub(Math.max(0, c.leftover));
    }
    if (editing) toast = 'Изменено: ' + op.title + ', ' + fmt(orig) + NB + sign;
    flash(toast);
    if (isPhone || editing) onClose();
    else {
      // На компьютере панель остаётся открытой для следующей записи.
      setE(defaultDraft(next));
      setQrText('');
      setError(null);
    }
  };

  const accent = isTrip ? 'var(--work)' : 'var(--own)';
  const style = { '--accent': accent, '--onAccent': isTrip ? 'var(--onWork)' : 'var(--onOwn)', '--tint': isTrip ? 'var(--workBg)' : 'var(--ownBg)' } as CSSProperties;

  return (
    <form className="sheet" aria-label="Новая операция" onSubmit={save} style={style}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
        <div style={{ flex: 1 }}>
          <Seg
            label="Тип операции"
            value={e.dir}
            onPick={pickDir}
            items={[
              { id: 'out', label: 'Расход' },
              { id: 'in', label: 'Доход' },
            ]}
          />
        </div>
        <button type="button" className="iconbtn" aria-label="Закрыть" onClick={onClose}>
          <Icon name="x" size={20} stroke={2} />
        </button>
      </div>

      {isPhone ? (
        <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'flex-start', minHeight: 70 }}>
          <p className="amountbig" aria-live="polite">
            {amountShown}
            <span style={{ marginLeft: 6, fontSize: 24, color: 'var(--ink2)' }}>{sign}</span>
          </p>
          <p className={'note' + (error ? ' warn' : '')} style={{ minHeight: 18, fontSize: 13, lineHeight: '18px' }}>
            {note}
          </p>
        </div>
      ) : (
        <>
          <label className="label">
            <span>Сумма, {sign}</span>
            <input
              className="field num"
              type="text"
              inputMode="decimal"
              autoComplete="off"
              autoFocus
              value={e.amount}
              onChange={(ev) => set({ amount: ev.target.value.replace(/\./g, ',').replace(/[^\d,]/g, '') })}
              placeholder="0"
              style={{ height: 60, color: accent, fontWeight: 300, fontSize: 32 }}
            />
          </label>
          <p className={'note' + (error ? ' warn' : '')} style={{ margin: '-8px 0 0', minHeight: 18, fontSize: 13, lineHeight: '18px' }}>
            {note}
          </p>
        </>
      )}

      <div className="chips scroll" role="group" aria-label="Чьи деньги">
        {scopes.map((s) => (
          <button key={s.id} type="button" aria-pressed={s.id === e.scope} className={'chip scope' + (isTripScope(s.id) ? ' trip' : '') + (s.id === e.scope ? ' sel' : '')} onClick={() => pickScope(s.id)}>
            {s.label}
          </button>
        ))}
        {hidden > 0 && (
          <button type="button" className="chip" onClick={() => setMoreTrips(true)}>
            Другие поездки: {hidden}
          </button>
        )}
      </div>
      <div className="chips scroll" role="group" aria-label="Категория">
        {catList(e.dir, e.scope).map((c) => (
          <button key={c} type="button" aria-pressed={c === e.cat} className={'chip' + (c === e.cat ? ' sel' : '')} onClick={() => set({ cat: c })}>
            {CATS[c].s}
          </button>
        ))}
      </div>
      <div className="chips scroll" role="group" aria-label="Счёт">
        {accounts.map((a) => (
          <button key={a.account.id} type="button" aria-pressed={a.account.id === e.acc} className={'chip' + (a.account.id === e.acc ? ' sel' : '')} style={{ minWidth: 44, padding: '0 10px' }} onClick={() => set({ acc: a.account.id })}>
            {a.account.short || a.account.name}
          </button>
        ))}
        {accounts.length === 0 && <p className="note warn">Нет счетов. Добавьте счёт в настройках.</p>}
      </div>

      <div style={{ display: 'grid', gridTemplateColumns: '138px minmax(0,1fr)', gap: 6 }}>
        <input className="field" type="date" aria-label="Дата" value={e.date} onChange={(ev) => onDate(ev.target.value)} style={{ height: 44, padding: '0 8px', fontSize: 14 }} />
        <input className="field" type="text" aria-label="Комментарий" placeholder="Комментарий" value={e.comment} onChange={(ev) => set({ comment: ev.target.value })} style={{ minWidth: 0, height: 44, fontSize: 14 }} />
      </div>

      {showDocs && (
        <div className="stack gap6">
          <div role="group" aria-label="Чек или документ" style={{ display: 'grid', gridTemplateColumns: 'repeat(3, minmax(0,1fr))', gap: 6 }}>
            {(
              [
                ['photo', 'Фото', 'camera'],
                ['qr', 'QR-код', 'qr'],
                ['file', 'Файл', 'clip'],
              ] as [DocKind, string, string][]
            ).map(([id, label, icon]) => (
              <button key={id} type="button" aria-pressed={e.receipt === id} className={'chip' + (e.receipt === id ? ' sel' : '')} style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', gap: 6, padding: '0 4px' }} onClick={() => pickReceipt(id)}>
                <Icon name={icon} size={17} />
                {label}
              </button>
            ))}
          </div>
          {e.att.length > 0 && <Thumbs items={e.att} onRemove={removeFile} />}
          {!e.receipt && !filesBusy && (
            <button type="button" className="btn link" style={{ alignSelf: 'flex-start', padding: 0, minHeight: 32, color: 'var(--own)', fontSize: 14 }} onClick={() => set({ receipt: 'photo' })}>
              Чек есть, сниму позже
            </button>
          )}
          {e.receipt === 'qr' && !mobile && !e.receiptNote && (
            <div style={{ display: 'grid', gridTemplateColumns: 'minmax(0,1fr) auto', gap: 6 }}>
              <input className="field" aria-label="Текст QR-кода с чека" placeholder="Вставьте текст QR-кода: t=…&s=…" value={qrText} onChange={(ev) => setQrText(ev.target.value)} style={{ height: 44, fontSize: 14 }} />
              <button type="button" className="btn ghost" disabled={!qrText.trim()} onClick={() => applyReceipt(qrText.trim())}>
                Разобрать
              </button>
            </div>
          )}
          <p className={'note' + (docWarn ? ' warn' : '')} style={{ fontSize: 13, lineHeight: '18px' }}>
            {docNote}
          </p>
        </div>
      )}
      {foodNote && (
        <p className="note" style={{ fontSize: 13, lineHeight: '18px' }}>
          {foodNote}
        </p>
      )}

      {isPhone && (
        <div className="keypad">
          {KEYS.map((k) => (
            <button key={k} type="button" aria-label={k === 'back' ? 'Стереть' : k === ',' ? 'Запятая' : k} onClick={() => pressKey(k)}>
              {k === 'back' ? <Icon name="back" size={22} /> : k}
            </button>
          ))}
        </div>
      )}

      <button type="submit" className="submit" disabled={busy}>
        {submitLabel}
      </button>
    </form>
  );
}
