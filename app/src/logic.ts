// Подписи и раскладка данных для экранов. Сами расчёты — в ядре.

import type { AccountView, Op, OpKind, Plan, Snapshot, TripStatus, TripView } from './api';
import { STATUSES } from './api';
import { NB, daysText, dMon, dRange, diffDays, fmt, fmt2, plural, rub } from './format';
import { ICON } from './icons';

export type Cat = { n: string; s: string; i: string };

export const CATS: Record<string, Cat> = {
  tickets: { n: 'Билеты', s: 'Билеты', i: 'tickets' },
  lodging: { n: 'Проживание', s: 'Проживание', i: 'lodging' },
  taxi: { n: 'Такси и транспорт', s: 'Такси', i: 'taxi' },
  car: { n: 'Аренда авто и топливо', s: 'Авто, топливо', i: 'car' },
  food: { n: 'Питание', s: 'Питание', i: 'food' },
  tools: { n: 'Материалы и инструменты', s: 'Материалы', i: 'tools' },
  groceries: { n: 'Продукты', s: 'Продукты', i: 'groceries' },
  cafe: { n: 'Кафе', s: 'Кафе', i: 'cafe' },
  transport: { n: 'Транспорт', s: 'Транспорт', i: 'transport' },
  home: { n: 'Дом', s: 'Дом', i: 'home' },
  telecom: { n: 'Связь и подписки', s: 'Связь', i: 'telecom' },
  gifts: { n: 'Подарки', s: 'Подарки', i: 'gifts' },
  other: { n: 'Другое', s: 'Другое', i: 'other' },
  salary: { n: 'Зарплата', s: 'Зарплата', i: 'salary' },
  interest: { n: 'Проценты', s: 'Проценты', i: 'interest' },
  otherIn: { n: 'Другой доход', s: 'Другое', i: 'other' },
  advance: { n: 'Аванс', s: 'Аванс', i: 'advance' },
  reimb: { n: 'Возмещение', s: 'Возмещение', i: 'reimb' },
  ret: { n: 'Возврат компании', s: 'Возврат', i: 'ret' },
  transfer: { n: 'Перевод между счетами', s: 'Перевод', i: 'transfer' },
  debt: { n: 'Долг', s: 'Долг', i: 'debt' },
};

export const TRIP_OUT = ['tickets', 'lodging', 'taxi', 'car', 'food', 'tools'];
export const TRIP_IN = ['advance', 'reimb'];
export const OWN_OUT = ['groceries', 'cafe', 'transport', 'home', 'telecom', 'gifts', 'other'];
export const OWN_IN = ['salary', 'interest', 'otherIn'];

export const STATUS = ['Заявка', 'Аванс получен', 'В поездке', 'Ожидает отчёт', 'На проверке', 'Закрыта'];
export const RAIL = ['Заявка', 'Аванс', 'Поездка', 'Отчёт', 'Проверка', 'Закрыта'];
export const NEXT = ['Отметить выдачу аванса', 'Начать поездку', 'Завершить поездку', 'Отправить на проверку', 'Закрыть поездку'];
export const ARTICLE: Record<string, string> = { tickets: '01', lodging: '03', taxi: '05', car: '05', tools: '05' };
export const DOC_LABEL: Record<string, string> = { photo: 'фото чека', qr: 'чек по QR-коду', file: 'файл' };

export const statusIndex = (s: TripStatus) => STATUSES.indexOf(s);
export const isIn = (o: Op) => o.kind === 'income' || o.kind === 'advance' || o.kind === 'reimbursement' || (o.kind === 'debt' && o.inflow);
export const isOwn = (o: Op) => o.kind === 'personal' || o.kind === 'income' || o.kind === 'debt' || o.kind === 'transfer';
/** Семейная трата: личный расход с меткой «Семья». */
export const isFamily = (o: Op) => o.kind === 'personal' && o.family && !o.trip;
export const isSpend = (o: Op) => o.kind === 'work' || o.kind === 'food' || o.kind === 'personal';

export function kindFor(dir: 'out' | 'in', isTrip: boolean, cat: string): OpKind {
  if (dir === 'out') return isTrip ? (cat === 'food' ? 'food' : 'work') : 'personal';
  return isTrip ? (cat === 'advance' ? 'advance' : 'reimbursement') : 'income';
}

export function accountOf(snap: Snapshot, id: string): AccountView | undefined {
  return snap.accounts.find((a) => a.account.id === id);
}

export type RailStep = { label: string; done: boolean; current: boolean };
export function rail(statusIdx: number): RailStep[] {
  return RAIL.map((label, i) => ({ label, done: i < statusIdx, current: i === statusIdx }));
}

export type OpView = {
  op: Op;
  title: string;
  icon: string;
  own: boolean;
  inc: boolean;
  amount: string;
  sub: string;
  dateSub: string;
  chip: string;
  chipTagged: boolean;
  /** Метка «семья». */
  chipFamily: boolean;
  noDoc: boolean;
  details: { label: string; value: string }[];
};

/** Перевод между своими счетами: не расход и не доход, «Мои деньги» не меняет. */
function transferView(snap: Snapshot, o: Op): OpView {
  const from = accountOf(snap, o.account);
  const to = o.to ? accountOf(snap, o.to) : undefined;
  const fromName = from ? from.account.name : 'Счёт удалён';
  const toName = to ? to.account.name : 'Счёт удалён';
  const fromCur = from?.account.currency || 'RUB';
  const toCur = to?.account.currency || 'RUB';
  const sent = fromCur !== 'RUB' && o.orig != null ? rub(o.orig, fromCur) : rub(o.amount);
  const got = toCur === 'RUB' ? rub(o.amount) : rub(o.to_amount ?? o.orig ?? o.amount, toCur);
  const details = [
    { label: 'Откуда', value: fromName },
    { label: 'Куда', value: toName },
  ];
  if (fromCur !== toCur) {
    details.push({ label: 'Ушло', value: sent });
    details.push({ label: 'Пришло', value: got });
  } else if (fromCur !== 'RUB') details.push({ label: 'В рублях', value: '≈' + NB + rub(o.amount) });
  if (o.note) details.push({ label: 'Примечание', value: o.note });
  const where = `${fromName} → ${toName}`;
  return {
    op: o,
    title: o.title,
    icon: ICON.transfer,
    own: true,
    inc: false,
    amount: sent,
    sub: where,
    dateSub: dMon(o.date) + ', ' + where.toLowerCase(),
    chip: '',
    chipTagged: false,
    chipFamily: false,
    noDoc: false,
    details,
  };
}

export function opView(snap: Snapshot, o: Op): OpView {
  if (o.kind === 'transfer') return transferView(snap, o);
  const acc = accountOf(snap, o.account);
  const accName = acc ? acc.account.name : 'Счёт удалён';
  const cat = o.kind === 'debt' ? CATS.debt : CATS[o.cat] || CATS.other;
  const inc = isIn(o);
  const tagged = o.kind === 'personal' && !!o.trip;
  const noDoc = o.kind === 'work' && !o.doc;
  const details = [
    { label: 'Категория', value: cat.n },
    { label: 'Счёт', value: accName },
  ];
  if (o.kind === 'work' && o.doc) {
    const art = ARTICLE[o.cat];
    details.push({ label: 'Документ', value: DOC_LABEL[o.doc] });
    details.push({ label: 'В отчёт 1С', value: art ? `03.01.04.${art}` : 'статью нужно уточнить' });
  }
  if (noDoc) details.push({ label: 'В отчёт 1С', value: 'не входит: из суточных, сверх них — долг' });
  if (o.kind === 'food') details.push({ label: 'В отчёт 1С', value: 'не идёт, за счёт суточных' });
  if (o.cur && o.cur !== 'RUB' && o.orig != null) details.push({ label: 'Сумма в валюте', value: rub(o.orig, o.cur) });
  if (o.kind === 'debt') {
    const d = snap.debts.find((v) => v.debt.id === o.debt);
    details.push({ label: 'Долг', value: d ? `${d.debt.who}, осталось ${rub(d.left)}` : 'долг удалён' });
  }
  if (o.plan) {
    const p = snap.plans.find((v) => v.plan.id === o.plan);
    details.push({ label: 'Регулярный платёж', value: (p ? p.plan.title : 'удалён') + (o.plan_date ? ', за ' + dMon(o.plan_date) : '') });
  }
  if (o.note) details.push({ label: 'Примечание', value: o.note });
  return {
    op: o,
    title: o.title,
    icon: ICON[cat.i] || ICON.other,
    own: isOwn(o),
    inc,
    amount: (inc ? '+' : '−') + rub(o.amount),
    sub: accName + (o.kind === 'food' ? ', за счёт суточных' : noDoc ? ', из суточных' : o.kind === 'debt' ? ', долг' : ''),
    dateSub: dMon(o.date) + ', ' + accName.toLowerCase() + (noDoc ? ', из суточных' : ''),
    chip: o.trip ? (tagged ? 'метка ' + o.trip : o.trip) : isFamily(o) ? 'семья' : '',
    chipTagged: tagged,
    chipFamily: isFamily(o),
    noDoc,
    details,
  };
}

const WEEK_ACC = ['воскресенье', 'понедельник', 'вторник', 'среду', 'четверг', 'пятницу', 'субботу'];
const WEEK_EACH = ['каждое', 'каждый', 'каждый', 'каждую', 'каждый', 'каждую', 'каждую'];

/** «каждое 10-е число», «каждую пятницу», «каждый год, 5 марта». */
export function everyText(p: Plan): string {
  const [y, m, d] = p.start.split('-').map(Number);
  if (p.every === 'week') {
    const wd = new Date(Date.UTC(y, m - 1, d)).getUTCDay();
    return `${WEEK_EACH[wd]} ${WEEK_ACC[wd]}`;
  }
  if (p.every === 'year') return 'каждый год, ' + dMon(p.start);
  return `каждое ${d}-е число`;
}

/** «сегодня», «завтра», «через 3 дня», «просрочен на 2 дня». */
export function dueText(days: number): string {
  if (days === 0) return 'сегодня';
  if (days === 1) return 'завтра';
  if (days > 1) return 'через ' + daysText(days);
  return 'просрочен на ' + daysText(-days);
}

export type TripRowView = {
  tv: TripView;
  name: string;
  dates: string;
  statusIdx: number;
  statusText: string;
  statusNote: string;
  balanceText: string;
  balanceNote: string;
  balanceWarn: boolean;
  docWarn: string;
  closed: boolean;
};

export function tripRow(tv: TripView): TripRowView {
  const t = tv.trip;
  const c = tv.s;
  const st = statusIndex(t.status);
  let note = STATUS[st];
  if (st === 2) note = c.elapsed > 0 ? `В поездке, ${c.elapsed}-й день из ${c.days}` : `В поездке, начало ${dMon(t.start)}`;
  if (st === 3 && t.deadline) note = 'Ожидает отчёт, сдать до ' + dMon(t.deadline);
  let balanceText = '';
  let balanceNote = 'рассчитано';
  let balanceWarn = false;
  if (c.balance > 0 && st <= 2) {
    balanceText = rub(c.balance);
    balanceNote = 'подотчётные';
  } else if (c.balance > 0) {
    balanceText = rub(c.balance);
    balanceNote = 'вернуть компании';
    balanceWarn = true;
  } else if (c.balance < 0) {
    balanceText = rub(-c.balance);
    balanceNote = 'компания должна';
  }
  const docWarn =
    c.no_doc_count && st < 5
      ? `${c.no_doc_count} ${plural(c.no_doc_count, 'расход', 'расхода', 'расходов')} без чека на ${rub(c.no_doc)}` + (c.debt > 0 ? `, долг ${rub(c.debt)}` : '')
      : '';
  return { tv, name: `${t.id} ${t.city}`, dates: dRange(t.start, t.end), statusIdx: st, statusText: STATUS[st], statusNote: note, balanceText, balanceNote, balanceWarn, docWarn, closed: st === 5 };
}

export type Line = { label: string; value: string };
export type Split = { label: string; value: string; warn: boolean };

export type TripSummary = {
  company: Line[];
  resultLabel: string;
  resultValue: string;
  resultWarn: boolean;
  split: Split[];
  companyNote: string;
  companyNoteWarn: boolean;
  canSettle: boolean;
  settleLabel: string;
  self: Line[];
  leftLabel: string;
  leftValue: string;
  leftWarn: boolean;
  leftNote: string;
  perDay: string;
  tagged: string;
  canSave: boolean;
  rateNote: string;
};

export function tripSummary(snap: Snapshot, tv: TripView): TripSummary {
  const c = tv.s;
  const rateR = fmt(tv.rate) + NB + '₽';
  const minus = (v: number) => (v ? '−' + rub(v) : rub(0));
  const company: Line[] = [{ label: 'Аванс по заявке', value: rub(c.issued) }];
  if (c.reimbursed) company.push({ label: 'Возмещено', value: rub(c.reimbursed) });
  company.push({ label: 'Расходы с документами', value: minus(c.confirmed) });
  company.push({ label: c.finished ? `Суточные, ${c.days} × ${rateR}` : `Суточные за ${daysText(c.elapsed)} × ${rateR}`, value: minus(c.per_diem) });
  if (c.returned) company.push({ label: 'Возвращено', value: minus(c.returned) });

  let resultLabel = 'Рассчитано';
  let resultValue = '';
  let resultWarn = false;
  if (c.balance > 0 && !c.finished) {
    resultLabel = 'Подотчётные сейчас';
    resultValue = rub(c.balance);
  } else if (c.balance > 0) {
    resultLabel = 'Вернуть компании';
    resultValue = rub(c.balance);
    resultWarn = true;
  } else if (c.balance < 0) {
    resultLabel = 'Компания должна вам';
    resultValue = rub(-c.balance);
  }
  const split: Split[] =
    c.debt_due > 0 && c.finished && c.balance > 0
      ? [
          { label: 'Остаток аванса', value: rub(c.balance - c.debt_due), warn: false },
          { label: 'Долг перед компанией', value: rub(c.debt_due), warn: true },
        ]
      : [];
  let companyNote = '';
  let companyNoteWarn = false;
  if (split.length) {
    companyNote = 'Долг — траты без документов, которые не покрыли суточные. Добавьте документы, и сумма уменьшится.';
    companyNoteWarn = true;
  } else if (!c.finished && c.debt > 0) {
    companyNote = `Траты без документов больше суточных за поездку: ${rub(c.debt_due || c.debt)} станет долгом перед компанией.`;
    companyNoteWarn = true;
  } else if (c.no_doc > 0) companyNote = `Расходы без документов ${rub(c.no_doc)} в отчёт не входят: вычитаются из суточных.`;
  else if (c.finished && c.balance < 0) companyNote = 'Перерасход подтверждён документами, компания возместит.';

  const self: Line[] = [{ label: (c.finished ? 'Суточные, ' : 'За всю поездку, ') + `${c.days} × ${rateR}`, value: rub(c.per_diem_full) }];
  if (c.food) self.push({ label: 'Питание', value: minus(c.food) });
  if (c.no_doc) self.push({ label: 'Расходы без документов', value: minus(c.no_doc) });
  let leftLabel: string;
  let leftValue: string;
  let leftWarn = false;
  let leftNote = '';
  if (c.debt_due > 0) {
    leftLabel = c.finished ? 'Долг перед компанией' : 'Будет долг перед компанией';
    leftValue = rub(c.debt_due);
    leftWarn = true;
    leftNote = (c.lost ? `Ещё ${rub(c.lost)} оплачено из своих, компания это не возместит. ` : '') + (c.finished ? 'Долг входит в сумму к возврату.' : 'Добавьте документы, и долга не будет.');
  } else if (c.debt > 0) {
    leftLabel = 'Компания не возместит';
    leftValue = rub(c.debt);
    leftWarn = true;
    leftNote = 'Траты сверх суточных оплачены из своих денег, возвращать нечего.';
  } else {
    leftLabel = c.finished ? 'Осталось от суточных' : 'Останется от суточных';
    leftValue = rub(c.leftover);
  }
  const perDay = !c.finished && c.leftover > 0 && c.days_left > 0 ? `≈ ${rub(Math.floor(c.leftover / c.days_left / 1000) * 1000)} в день на оставшиеся ${daysText(c.days_left)}` : '';
  const rateNote = tv.rate_source === 'own' ? `Своя для поездки, в настройках ${rub(snap.rate)}` : tv.rate_source === 'fixed' ? 'Как в сданном отчёте' : 'Как в настройках';
  return {
    company,
    resultLabel,
    resultValue,
    resultWarn,
    split,
    companyNote,
    companyNoteWarn,
    canSettle: c.finished && c.balance !== 0,
    settleLabel: c.balance < 0 ? 'Возмещение получено' : 'Деньги возвращены',
    self,
    leftLabel,
    leftValue,
    leftWarn,
    leftNote,
    perDay,
    tagged: c.tagged > 0 ? rub(c.tagged) : '',
    canSave: false,
    rateNote,
  };
}

export type Control = { label: string; value: string; warn: boolean; go: 'report-plan' | 'report-fact' | 'docs' | 'summary' };

export function tripControls(snap: Snapshot, tv: TripView, ops: Op[]): Control[] {
  const t = tv.trip;
  const c = tv.s;
  const st = statusIndex(t.status);
  const work = ops.filter((o) => o.trip === t.id && o.kind === 'work');
  const withDoc = work.filter((o) => o.doc).length;
  const sent = work.filter((o) => o.doc && o.sent).length;
  const out: Control[] = [];
  if (st === 3 && t.deadline) {
    const left = diffDays(snap.today, t.deadline);
    out.push({ label: 'Срок отчёта, ' + dMon(t.deadline), value: left >= 0 ? 'осталось ' + daysText(left) : 'просрочен', warn: left <= 3, go: 'report-fact' });
  }
  out.push({ label: 'Расходы без документов', value: c.no_doc_count ? `${c.no_doc_count} на ${rub(c.no_doc)}` : 'нет', warn: c.no_doc_count > 0, go: 'docs' });
  if (tv.over_total > 0 && c.finished) out.push({ label: 'Сверх плана', value: `+${rub(tv.over_total)}, нужно обоснование`, warn: false, go: 'report-plan' });
  if (withDoc) out.push({ label: 'Отправлено в офис', value: `${sent} из ${withDoc}`, warn: false, go: 'docs' });
  if (c.finished && c.balance < 0) out.push({ label: 'Возмещение от компании', value: 'не поступило', warn: false, go: 'summary' });
  if (c.finished && c.balance > 0) out.push({ label: 'Возврат компании', value: 'не отмечен', warn: true, go: 'summary' });
  return out;
}

export { fmt2, rub };
