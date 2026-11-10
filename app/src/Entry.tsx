import { useEffect, useState, type CSSProperties, type FormEvent } from 'react';
import { api, inApp, message, scanQr, type Account, type Attachment, type DocKind, type FxQuote, type Op, type Snapshot, type TripView } from './api';
import { NB, SIGN, amountText, dMon, fmt, parseAmount, plural, rub } from './format';
import { attachFile, pickFiles } from './media';
import { CATS, OWN_IN, OWN_OUT, TRIP_IN, TRIP_OUT, catFromHistory, isFamily, isIn, kindFor, statusIndex } from './logic';
import { aiOn, useAi } from './aiState';
import { Icon, Seg, Thumbs } from './ui';

export type EntryDraft = {
  /** Расход, доход или перевод между своими счетами. */
  dir: 'out' | 'in' | 'move';
  amount: string;
  /** 'own' — личное, 'family' — семейное, 'trip:573' — расход поездки, 'tag:573' — личное с меткой поездки. */
  scope: string;
  cat: string;
  acc: string;
  /** Перевод: счёт, на который ушли деньги. */
  to: string;
  /** Перевод между валютами: сколько пришло на счёт «Куда». */
  toAmount: string;
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
    to: '',
    toAmount: '',
    receipt: null,
    receiptNote: '',
    att: [],
    comment: '',
    date: snap.today,
  };
}

/** Куда обычно переводят: с карты — в наличные, с наличных — на личную карту. */
function defaultTo(snap: Snapshot, from: string): string {
  const live = snap.accounts.filter((a) => !a.account.archived && a.account.id !== from);
  const want = snap.accounts.find((a) => a.account.id === from)?.account.kind === 'cash' ? 'personal' : 'cash';
  return (live.find((a) => a.account.kind === want) || live[0])?.account.id ?? '';
}

const TRANSFER_TITLES = ['Перевод между счетами', 'Снятие наличных', 'Наличные на карту', 'Обмен валюты'];

function transferTitle(from?: Account, to?: Account): string {
  if (from && to && from.currency !== to.currency) return 'Обмен валюты';
  if (to?.kind === 'cash' && from?.kind !== 'cash') return 'Снятие наличных';
  if (from?.kind === 'cash' && to && to.kind !== 'cash') return 'Наличные на карту';
  return 'Перевод между счетами';
}

/** Черновик из записанной операции — чтобы её поправить. */
export function draftFromOp(o: Op): EntryDraft {
  if (o.kind === 'transfer') {
    return {
      dir: 'move',
      amount: amountText(o.orig ?? o.amount),
      scope: 'own',
      cat: 'transfer',
      acc: o.account,
      to: o.to ?? '',
      // Из валюты в рубли пришло столько, сколько записано в рублях.
      toAmount: o.to_amount != null ? amountText(o.to_amount) : o.orig != null ? amountText(o.amount) : '',
      receipt: null,
      receiptNote: '',
      att: [],
      comment: TRANSFER_TITLES.includes(o.title) ? '' : o.title,
      date: o.date,
    };
  }
  const tripKind = o.kind === 'work' || o.kind === 'food' || o.kind === 'advance' || o.kind === 'reimbursement';
  const cat = o.kind === 'advance' ? 'advance' : o.kind === 'reimbursement' ? 'reimb' : o.cat;
  const catName = (CATS[cat] || CATS.other).n;
  return {
    dir: isIn(o) ? 'in' : 'out',
    amount: amountText(o.orig ?? o.amount),
    scope: o.trip ? (tripKind ? 'trip:' : 'tag:') + o.trip : isFamily(o) ? 'family' : 'own',
    cat,
    acc: o.account,
    to: '',
    toAmount: '',
    receipt: o.doc,
    receiptNote: o.doc === 'qr' ? o.note : '',
    att: o.att ?? [],
    comment: o.title === catName ? '' : o.title,
    date: o.date,
  };
}

const isTripScope = (s: string) => s.startsWith('trip:');
const scopeTrip = (s: string) => (s.includes(':') ? s.slice(s.indexOf(':') + 1) : null);

function catList(dir: EntryDraft['dir'], scope: string): string[] {
  if (dir === 'move') return ['transfer'];
  if (isTripScope(scope)) return dir === 'out' ? TRIP_OUT : TRIP_IN;
  return dir === 'in' ? OWN_IN : OWN_OUT;
}

/** Курсы на дату, уже полученные в этом сеансе. */
const quotes = new Map<string, FxQuote>();

/**
 * Курс валюты на дату операции: курс ЦБ (с сайта ЦБ или сохранённый), а без него — из настроек.
 * Пока курс на дату загружается, — последний известный.
 */
function useRate(snap: Snapshot, cur: string, date: string): FxQuote {
  const key = cur + '|' + date;
  const auto = snap.fx_auto && cur !== 'RUB';
  const [got, setGot] = useState<{ key: string; q: FxQuote } | null>(null);
  useEffect(() => {
    if (!auto || !inApp || quotes.has(key)) return;
    let alive = true;
    api.fxRate(cur, date).then(
      (q) => {
        // Курс на эту дату больше не изменится; последние сохранённые (без связи) — не запоминаем.
        if (q.source === 'cbr' && q.date && !q.fallback) quotes.set(key, q);
        if (alive) setGot({ key, q });
      },
      () => undefined,
    );
    return () => {
      alive = false;
    };
  }, [auto, key, cur, date]);
  if (cur === 'RUB') return { rate: 1, source: 'cbr', date: null };
  const manual: FxQuote = snap.fx_manual[cur] ? { rate: snap.fx_manual[cur], source: 'manual', date: null } : { rate: 0, source: 'none', date: null };
  if (!auto) return manual;
  if (got && got.key === key) return got.q;
  const cached = quotes.get(key);
  if (cached) return cached;
  return snap.fx_date && snap.fx[cur] ? { rate: snap.fx[cur], source: 'cbr', date: snap.fx_date } : manual;
}

function rateSource(q: FxQuote): string {
  return q.source === 'cbr' ? 'по курсу ЦБ' + (q.date ? ' на ' + dMon(q.date) : '') : 'по курсу из настроек';
}

/** «≈ 2 774,40 ₽ по курсу ЦБ на 6 ноября» */
function rubNote(q: FxQuote, cur: string, rubAmount: number): string {
  if (!q.rate) return `Нет курса для ${cur}: укажите его в настройках`;
  return `≈${NB}${rub(rubAmount)} ${rateSource(q)}`;
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
  const ai = useAi();
  const [phrase, setPhrase] = useState('');
  const [aiBusy, setAiBusy] = useState<'phrase' | 'doc' | null>(null);
  /** Что сделал Claude: «заполнено по фразе», сомнения. */
  const [aiNote, setAiNote] = useState<{ text: string; warn: boolean } | null>(null);
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

  const accounts = snap.accounts.filter((a) => !a.account.archived || a.account.id === e.acc || a.account.id === e.to);
  const acc = accounts.find((a) => a.account.id === e.acc) || null;
  const cur = acc ? acc.account.currency : 'RUB';
  const sign = SIGN[cur] || cur;
  const move = e.dir === 'move';
  const toAcc = move ? accounts.find((a) => a.account.id === e.to) || null : null;
  const toCur = toAcc ? toAcc.account.currency : 'RUB';
  const toSign = SIGN[toCur] || toCur;
  const isTrip = !move && isTripScope(e.scope);
  const tripId = move ? null : scopeTrip(e.scope);
  const family = !move && e.dir === 'out' && e.scope === 'family';
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
  const pickDir = (dir: EntryDraft['dir']) => {
    if (dir === 'move') return set({ dir, cat: 'transfer', receipt: null, to: e.to && e.to !== e.acc ? e.to : defaultTo(snap, e.acc) });
    // Семейные бывают только расходы.
    const scope = dir === 'in' && e.scope === 'family' ? 'own' : e.scope;
    set({ dir, scope, cat: catList(dir, scope)[0], receipt: dir === 'out' ? e.receipt : null });
  };
  /** Перевод: счёт «Откуда». Тот же счёт в «Куда» меняется местами с прежним. */
  const pickFrom = (id: string) => set(id === e.to ? { acc: id, to: e.acc, toAmount: '' } : { acc: id, toAmount: '' });
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
    if (move || e.scope.startsWith('tag:') || e.scope === 'family') return set({ date: d });
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

  /** Фраза → форма (Claude Haiku 5.5). Записывает человек кнопкой ниже. */
  const fillFromPhrase = async () => {
    const text = phrase.trim();
    if (!text || aiBusy) return;
    setAiBusy('phrase');
    setAiNote(null);
    try {
      const r = await api.aiPhrase(text);
      const acc2 = r.acc || e.acc;
      const tripOut = r.dir === 'out' && isTripScope(r.scope);
      setE((x) => ({
        ...x,
        dir: r.dir,
        amount: r.amount > 0 ? amountText(Math.round(r.amount * 100)) : x.amount,
        scope: r.dir === 'move' ? 'own' : r.scope,
        cat: r.cat,
        acc: acc2,
        to: r.dir === 'move' ? r.to || defaultTo(snap, acc2) : '',
        toAmount: '',
        receipt: tripOut && r.cat !== 'food' ? (r.document ? x.receipt || 'photo' : x.att.length ? x.receipt : null) : null,
        receiptNote: tripOut ? x.receiptNote : '',
        comment: r.comment || x.comment,
        date: r.date || x.date,
      }));
      setPhrase('');
      setError(null);
      setAiNote(r.unclear ? { text: 'Claude не уверен: ' + r.unclear, warn: true } : { text: r.amount > 0 ? 'Заполнено по фразе — проверьте и запишите' : 'Заполнено по фразе — впишите сумму', warn: r.amount <= 0 });
    } catch (err) {
      setAiNote({ text: message(err), warn: true });
    } finally {
      setAiBusy(null);
    }
  };
  /** Билет, счёт гостиницы или чек → сумма, дата, статья, комментарий (Claude Opus 5.5). */
  const fillFromDoc = async () => {
    if (!e.att.length || aiBusy) return;
    setAiBusy('doc');
    setAiNote(null);
    try {
      const r = await api.aiDocScan(
        e.att.map((a) => a.id),
        tripId,
      );
      const patch: Partial<EntryDraft> = {};
      if (r.amount > 0) patch.amount = amountText(Math.round(r.amount * 100));
      if (r.date) patch.date = r.date;
      if (TRIP_OUT.includes(r.cat)) patch.cat = r.cat;
      if (r.comment) patch.comment = r.comment;
      set(patch);
      const other = r.currency && r.currency !== cur ? `Сумма в документе — в ${r.currency}, а счёт — в ${cur}: проверьте счёт. ` : '';
      setAiNote(r.problems ? { text: other + 'Claude не уверен: ' + r.problems, warn: true } : { text: other + 'Заполнено по документу — проверьте и запишите', warn: !!other });
    } catch (err) {
      setAiNote({ text: message(err), warn: true });
    } finally {
      setAiBusy(null);
    }
  };

  // Курсы на дату операции: для счёта в валюте и для счёта, куда идёт перевод.
  const qFrom = useRate(snap, cur, e.date || snap.today);
  const qTo = useRate(snap, move ? toCur : 'RUB', e.date || snap.today);
  const fx = qFrom.rate;

  // Подсказки под суммой и под чеком.
  const amt = parseAmount(e.amount) ?? 0;
  const amtRub = Math.round(amt * fx);
  // Перевод между валютами: сколько пришло (если не указано — по курсу).
  const cross = move && !!toAcc && toCur !== cur;
  const toAmt = cross ? parseAmount(e.toAmount) : null;
  const estTo = cross && qFrom.rate && qTo.rate ? Math.round((amt * qFrom.rate) / qTo.rate) : null;
  let note = '';
  if (error) note = error;
  else if (e.receipt === 'qr' && e.receiptNote) note = e.receiptNote.split('.')[0];
  else if (cross) {
    // Из рублей — курс валюты «Куда», в рубли — курс валюты «Откуда», между двумя валютами — оба.
    const q: FxQuote = cur === 'RUB' ? qTo : toCur === 'RUB' ? qFrom : qFrom.source === 'cbr' && qTo.source === 'cbr' ? qFrom : { ...qFrom, source: 'manual' };
    if (amt > 0 && !toAmt) note = estTo != null ? `Придёт ≈${NB}${fmt(estTo)}${NB}${toSign} ${rateSource(q)}` : `Укажите, сколько пришло, ${toSign}`;
  } else if (cur !== 'RUB' && amt > 0) note = rubNote(qFrom, cur, amtRub);
  else if (move) note = '«Мои деньги» не меняются: деньги переходят между вашими счетами';

  // Бюджет месяца: сколько останется в лимите категории (или в общем) после этой покупки.
  let budgetNote = '';
  let budgetWarn = false;
  if (!move && e.dir === 'out' && !isTrip) {
    const day = e.date || snap.today;
    const bv = snap.budgets.find((b) => b.id === (family ? 'family' : 'personal'));
    if (bv && day >= bv.start && day <= bv.end) {
      // При правке прежняя сумма этой операции уже в «потрачено».
      const back = editing && editing.kind === 'personal' && (editing.family ? 'family' : 'personal') === bv.id && editing.date >= bv.start && editing.date <= bv.end ? editing : null;
      const cv = bv.cats.find((c) => c.cat === e.cat);
      const left = (limit: number, spent: number) => (spent <= limit ? 'останется ' + rub(limit - spent) : 'сверх лимита на ' + rub(spent - limit)) + ' из ' + rub(limit);
      if (cv && cv.limit != null) {
        const spent = cv.spent - (back && back.cat === e.cat ? back.amount : 0) + amtRub;
        budgetNote = `${(CATS[e.cat] || CATS.other).n}: ${left(cv.limit, spent)}`;
        budgetWarn = spent > cv.limit;
      } else if (bv.total) {
        const spent = bv.spent - (back ? back.amount : 0) + amtRub;
        budgetNote = `${family ? 'Семейный бюджет' : 'Бюджет месяца'}: ${left(bv.total, spent)}`;
        budgetWarn = spent > bv.total;
      }
    }
  }

  // Подсказка категории по прошлым операциям с тем же названием — без ИИ.
  const histCat = move || e.dir === 'move' ? null : catFromHistory(snap.ops, e.comment, e.dir, isTrip);
  const showHist = !!histCat && histCat !== e.cat && catList(e.dir, e.scope).includes(histCat);

  const room = tv ? Math.max(0, tv.s.leftover) : 0;
  const showDocs = !move && e.dir === 'out' && isTrip && e.cat !== 'food';
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

  const submitLabel = editing
    ? 'Сохранить изменения'
    : move
      ? 'Записать перевод'
      : isTrip
        ? (e.dir === 'out' ? 'Записать в поездку ' : 'Записать доход в поездку ') + tripId
        : family
          ? 'Записать в семейные траты'
          : e.dir === 'out'
            ? 'Записать в личное'
            : 'Записать доход в личное';

  // Сначала текущая поездка, личное и метка; предстоящие поездки — сразу, сданные на отчёт — по кнопке.
  const tripLabel = (t: TripView) => `${t.trip.id} ${t.trip.city}`;
  const scopes: { id: string; label: string }[] = [];
  if (active) scopes.push({ id: 'trip:' + active.trip.id, label: tripLabel(active) });
  scopes.push({ id: 'own', label: 'Личное' });
  if (e.dir === 'out') scopes.push({ id: 'family', label: 'Семья' });
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

  /** Перевод между своими счетами. */
  const saveMove = async () => {
    const orig = parseAmount(e.amount);
    if (!orig || orig <= 0) return setError('Введите сумму');
    if (!acc || !toAcc) return setError('Выберите, откуда и куда');
    if (acc.account.id === toAcc.account.id) return setError('Счета «Откуда» и «Куда» должны быть разными');
    const got = cross ? (toAmt ?? estTo) : null;
    if (cross && (!got || got <= 0)) return setError(`Укажите, сколько пришло, ${toSign}`);
    // В рублях: из рублей — сколько ушло, в рубли — сколько пришло, между валютами — по курсу (при правке без изменений — как было).
    const keep = !!editing && editing.kind === 'transfer' && editing.orig === orig && editing.date === (e.date || snap.today) && editing.account === acc.account.id && editing.to === toAcc.account.id;
    const amount = cur === 'RUB' ? orig : toCur === 'RUB' && got ? got : keep && editing ? editing.amount : Math.round(orig * fx);
    if (!amount) return setError(`Нет курса для ${cur}: укажите его в настройках`);
    const op: Op = {
      // Поля, которых нет в форме (из более новой версии, снимки), при правке не теряются.
      ...(editing ?? {}),
      id: editing ? editing.id : '',
      date: e.date || snap.today,
      kind: 'transfer',
      cat: 'transfer',
      title: e.comment.trim() || transferTitle(acc.account, toAcc.account),
      amount,
      account: acc.account.id,
      trip: null,
      doc: null,
      sent: false,
      family: false,
      debt: null,
      inflow: false,
      orig: cur === 'RUB' ? null : orig,
      cur: cur === 'RUB' ? null : cur,
      note: editing ? editing.note : '',
      to: toAcc.account.id,
      to_amount: toCur === 'RUB' ? null : toCur === cur ? orig : got,
    };
    setBusy(true);
    const next = await onSave(op);
    setBusy(false);
    if (!next) return;
    const where = `${acc.account.name} → ${toAcc.account.name}`;
    flash((editing ? 'Изменено: ' : 'Перевод записан: ') + `${fmt(orig)}${NB}${sign}, ${where}`);
    if (isPhone || editing) onClose();
    else {
      // На компьютере панель остаётся открытой для следующего перевода.
      setE({ ...defaultDraft(next), dir: 'move', cat: 'transfer', acc: op.account, to: op.to ?? '' });
      setError(null);
    }
  };

  const save = async (ev?: FormEvent) => {
    ev?.preventDefault();
    if (busy) return;
    if (move) return saveMove();
    const orig = parseAmount(e.amount);
    if (!orig || orig <= 0) return setError('Введите сумму');
    if (!acc) return setError('Выберите счёт');
    if (cur !== 'RUB' && !fx) return setError(`Нет курса для ${cur}: укажите его в настройках`);
    const kind = kindFor(e.dir === 'in' ? 'in' : 'out', isTrip, e.cat);
    const doc = kind === 'work' ? e.receipt : null;
    // Сумма в рублях: при правке без изменений — как была, иначе по курсу на дату операции.
    const same = !!editing && editing.orig === (cur === 'RUB' ? null : orig) && editing.date === (e.date || snap.today) && editing.account === acc.account.id;
    const amount = cur === 'RUB' ? orig : same && editing ? editing.amount : Math.round(orig * fx);
    const op: Op = {
      // Поля, которых нет в форме (из более новой версии, оплата по графику), при правке не теряются.
      ...(editing ?? {}),
      id: editing ? editing.id : '',
      date: e.date || snap.today,
      kind,
      cat: e.cat,
      title: e.comment.trim() || (CATS[e.cat] || CATS.other).n,
      amount,
      account: acc.account.id,
      trip: tripId,
      doc,
      sent: !!editing && editing.sent && !!doc,
      family: family && kind === 'personal',
      debt: editing ? editing.debt : null,
      inflow: editing ? editing.inflow : false,
      orig: cur === 'RUB' ? null : orig,
      cur: cur === 'RUB' ? null : cur,
      note: doc === 'qr' ? e.receiptNote : editing && editing.doc !== 'qr' ? editing.note : '',
      att: e.att,
      to: null,
      to_amount: null,
    };
    setBusy(true);
    const next = await onSave(op);
    setBusy(false);
    if (!next) return;
    let toast = `Записано: ${fmt(orig)}${NB}${sign}, ` + (isTrip ? 'поездка ' + tripId : e.scope.startsWith('tag:') ? 'личное, метка ' + tripId : family ? 'семья' : 'личное');
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
              { id: 'move', label: 'Перевод' },
            ]}
          />
        </div>
        <button type="button" className="iconbtn" aria-label="Закрыть" onClick={onClose}>
          <Icon name="x" size={20} stroke={2} />
        </button>
      </div>

      {aiOn(ai, 'phrase') && !editing && (
        <div className="phrase">
          <input
            className="field"
            type="text"
            aria-label="Фразой"
            placeholder="Фразой: такси 640 до объекта, чек есть"
            value={phrase}
            onChange={(ev) => setPhrase(ev.target.value)}
            onKeyDown={(ev) => {
              // Enter в строке фразы заполняет форму, а не записывает операцию.
              if (ev.key === 'Enter') {
                ev.preventDefault();
                fillFromPhrase();
              }
            }}
          />
          <button type="button" className="btn ghost" aria-label="Заполнить по фразе" disabled={!phrase.trim() || !!aiBusy} onClick={fillFromPhrase}>
            <Icon name="spark" size={18} />
            {aiBusy === 'phrase' ? 'Claude…' : 'Заполнить'}
          </button>
        </div>
      )}
      {aiNote && aiBusy !== 'doc' && (
        <p className={'note' + (aiNote.warn ? ' warn' : '')} role="status" style={{ fontSize: 13, lineHeight: '18px', margin: '-2px 0 0' }}>
          {aiNote.text}
        </p>
      )}

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

      {move ? (
        <>
          <div className="label">
            <span>Откуда</span>
            <div className="chips scroll" role="group" aria-label="Откуда">
              {accounts.map((a) => (
                <button key={a.account.id} type="button" aria-pressed={a.account.id === e.acc} className={'chip' + (a.account.id === e.acc ? ' sel' : '')} style={{ minWidth: 44, padding: '0 10px' }} onClick={() => pickFrom(a.account.id)}>
                  {a.account.short || a.account.name}
                </button>
              ))}
            </div>
          </div>
          <div className="label">
            <span>Куда</span>
            <div className="chips scroll" role="group" aria-label="Куда">
              {accounts
                .filter((a) => a.account.id !== e.acc)
                .map((a) => (
                  <button key={a.account.id} type="button" aria-pressed={a.account.id === e.to} className={'chip' + (a.account.id === e.to ? ' sel' : '')} style={{ minWidth: 44, padding: '0 10px' }} onClick={() => set({ to: a.account.id, toAmount: '' })}>
                    {a.account.short || a.account.name}
                  </button>
                ))}
            </div>
            {accounts.length < 2 && <p className="note warn">Для перевода нужно два счёта. Добавьте счёт в настройках.</p>}
          </div>
          {cross && (
            <label className="label">
              <span>Пришло, {toSign}</span>
              <input
                className="field num"
                type="text"
                inputMode="decimal"
                autoComplete="off"
                value={e.toAmount}
                onChange={(ev) => set({ toAmount: ev.target.value.replace(/\./g, ',').replace(/[^\d,]/g, '') })}
                placeholder={estTo ? amountText(estTo) : '0'}
                style={{ height: 44, fontSize: 18 }}
              />
            </label>
          )}
        </>
      ) : (
        <>
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
          {budgetNote && (
            <p className={'note' + (budgetWarn ? ' warn' : '')} style={{ fontSize: 13, lineHeight: '18px' }}>
              {budgetNote}
            </p>
          )}
        </>
      )}

      <div style={{ display: 'grid', gridTemplateColumns: '138px minmax(0,1fr)', gap: 6 }}>
        <input className="field" type="date" aria-label="Дата" value={e.date} onChange={(ev) => onDate(ev.target.value)} style={{ height: 44, padding: '0 8px', fontSize: 14 }} />
        <input className="field" type="text" aria-label="Комментарий" placeholder="Комментарий" value={e.comment} onChange={(ev) => set({ comment: ev.target.value })} style={{ minWidth: 0, height: 44, fontSize: 14 }} />
      </div>
      {showHist && histCat && (
        <div className="chips">
          <button type="button" className="chip" onClick={() => set({ cat: histCat })}>
            Как раньше: {CATS[histCat].n}
          </button>
        </div>
      )}

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
          {e.att.length > 0 && aiOn(ai, 'docs') && (
            <button type="button" className="btn ghost small" style={{ alignSelf: 'flex-start', display: 'flex', alignItems: 'center', gap: 6 }} disabled={!!aiBusy || filesBusy} onClick={fillFromDoc}>
              <Icon name="spark" size={16} />
              {aiBusy === 'doc' ? 'Claude читает документ…' : 'Распознать: сумма, дата, статья'}
            </button>
          )}
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
