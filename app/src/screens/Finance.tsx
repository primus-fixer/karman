import { useState, type ReactNode } from 'react';
import { api, type Debt, type DebtDir, type DebtView, type Due, type Every, type Plan, type PlanView, type Snapshot } from '../api';
import { NB, SIGN, amountText, dMon, parseAmount, rub } from '../format';
import { CATS, OWN_OUT, OWN_OUT_ALL, dueText, everyText, opView, topCats } from '../logic';
import { ErrorText, Icon, Seg } from '../ui';
import BudgetScreen from './Budget';
import Savings from './Savings';
import StatsScreen from './Stats';

/** Разделы «Финансов». */
export type FinView = 'pay' | 'budget' | 'save' | 'stats';

/** Действия экрана: запись через ядро и сообщение внизу. */
export type FinanceActions = {
  run: (p: Promise<Snapshot>, ok?: string) => Promise<Snapshot | null>;
  /** После первого платежа или долга со сроком — попросить разрешение на уведомления (Android). */
  saved: () => void;
  /** Сообщение внизу экрана (`long` — подольше, например с путём к файлу). */
  flash: (text: string, long?: boolean) => void;
  /** Хранилище закрылось — на экран входа. */
  locked: () => void;
};

function newId(): string {
  const b = new Uint8Array(8);
  crypto.getRandomValues(b);
  return Array.from(b, (x) => x.toString(16).padStart(2, '0')).join('');
}

function addMonths(iso: string, n: number): string {
  const [y, m, d] = iso.split('-').map(Number);
  const t = new Date(Date.UTC(y, m - 1 + n, 1));
  const last = new Date(Date.UTC(t.getUTCFullYear(), t.getUTCMonth() + 1, 0)).getUTCDate();
  t.setUTCDate(Math.min(d, last));
  return t.toISOString().slice(0, 10);
}

const REMIND: { id: string; label: string }[] = [
  { id: '', label: 'Как в настройках' },
  { id: '0', label: 'В день платежа' },
  { id: '1', label: 'За 1 день' },
  { id: '3', label: 'За 3 дня' },
  { id: '7', label: 'За неделю' },
];

const live = (snap: Snapshot) => snap.accounts.filter((a) => !a.account.archived);
/** Долг ещё не возвращён и не закрыт. */
export const isOpen = (v: DebtView) => !v.debt.closed && v.left > 0;
const accountByKind = (snap: Snapshot, kind: string) => (live(snap).find((a) => a.account.kind === kind) || live(snap)[0])?.account.id ?? '';

function AccountChips({ snap, value, onPick, label }: { snap: Snapshot; value: string; onPick: (id: string) => void; label: string }) {
  return (
    <div className="chips scroll" role="group" aria-label={label}>
      {live(snap).map((a) => (
        <button key={a.account.id} type="button" aria-pressed={a.account.id === value} className={'chip' + (a.account.id === value ? ' sel' : '')} style={{ minWidth: 44, padding: '0 10px' }} onClick={() => onPick(a.account.id)}>
          {a.account.short || a.account.name}
        </button>
      ))}
    </div>
  );
}

function Check({ checked, onChange, children }: { checked: boolean; onChange: (v: boolean) => void; children: ReactNode }) {
  return (
    <label style={{ display: 'flex', alignItems: 'center', gap: 10, minHeight: 44 }}>
      <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} style={{ flex: 'none', width: 20, height: 20, margin: 0, accentColor: 'var(--own)' }} />
      <span>{children}</span>
    </label>
  );
}

/** Строка ближайшего срока: платёж по графику или возврат долга. */
export function DueRow({ snap, due, act, compact = false }: { snap: Snapshot; due: Due; act: FinanceActions; compact?: boolean }) {
  const [paying, setPaying] = useState(false);
  const late = due.days < 0;
  const plan = due.kind === 'plan' ? snap.plans.find((p) => p.plan.id === due.id) : undefined;
  const debt = due.kind === 'debt' ? snap.debts.find((d) => d.debt.id === due.id) : undefined;
  const what =
    due.kind === 'plan' ? (due.auto ? 'спишется само' : plan ? everyText(plan.plan) : '') : due.kind === 'deposit' ? 'заканчивается вклад, будет ≈' + NB + rub(due.amount) : due.i_owe ? 'вернуть долг' : 'вам вернут долг';
  return (
    <div className="duerow">
      <div className="between" style={{ alignItems: 'flex-start' }}>
        <span className="stack" style={{ minWidth: 0 }}>
          <span className="optitle">{due.title}</span>
          <span className={'opsub' + (late ? ' warn' : '')}>
            {dueText(due.days)}, {dMon(due.date)} · {what}
          </span>
        </span>
        <span className="amount" style={{ flex: 'none' }}>
          {rub(due.amount)}
        </span>
      </div>
      {!paying && (
        <div className="wrap" style={{ gap: 6 }}>
          {due.kind === 'plan' && !due.auto && (
            <button type="button" className="btn own small" onClick={() => act.run(api.planPay(due.id, due.date, null, null), `Оплачено: ${due.title}`)}>
              Оплачено
            </button>
          )}
          {due.kind === 'plan' && due.auto && late && (
            <button type="button" className="btn own small" onClick={() => act.run(api.planPay(due.id, due.date, null, null), `Списание записано: ${due.title}`)}>
              Списалось
            </button>
          )}
          {due.kind === 'plan' && (
            <button type="button" className="btn ghost small" onClick={() => act.run(api.planSkip(due.id, due.date), `Пропущено: ${due.title}, ${dMon(due.date)}`)}>
              Пропустить
            </button>
          )}
          {due.kind === 'debt' && debt && !compact && (
            <button type="button" className="btn own small" onClick={() => setPaying(true)}>
              Записать возврат
            </button>
          )}
        </div>
      )}
      {paying && debt && <DebtPay snap={snap} view={debt} act={act} onDone={() => setPaying(false)} />}
    </div>
  );
}

/** Возврат долга: сколько, на какой счёт (или с какого), когда. */
function DebtPay({ snap, view, act, onDone }: { snap: Snapshot; view: DebtView; act: FinanceActions; onDone: () => void }) {
  const iOwe = view.debt.dir === 'i_owe';
  const [amount, setAmount] = useState(amountText(view.left));
  const [account, setAccount] = useState(accountByKind(snap, 'personal'));
  const [date, setDate] = useState(snap.today);
  const [error, setError] = useState<string | null>(null);
  const save = async () => {
    const k = parseAmount(amount);
    if (!k || k <= 0) return setError('Введите сумму');
    if (!account) return setError('Выберите счёт');
    const s = await act.run(api.debtPay(view.debt.id, k, account, date || snap.today), `Возврат записан: ${rub(k)}`);
    if (s) onDone();
  };
  return (
    <div className="stack gap10" style={{ paddingTop: 6 }}>
      <div className="form-grid">
        <label className="label">
          <span>Сумма, ₽</span>
          <input className="field num" inputMode="decimal" value={amount} onChange={(e) => setAmount(e.target.value)} autoFocus />
        </label>
        <label className="label">
          <span>Дата</span>
          <input className="field" type="date" value={date} onChange={(e) => setDate(e.target.value)} />
        </label>
      </div>
      <span className="note">{iOwe ? 'С какого счёта отдаёте' : 'На какой счёт пришли деньги'}</span>
      <AccountChips snap={snap} value={account} onPick={setAccount} label={iOwe ? 'С какого счёта' : 'На какой счёт'} />
      <ErrorText text={error} />
      <div className="wrap">
        <button type="button" className="btn own" onClick={save}>
          Записать возврат
        </button>
        <button type="button" className="btn ghost" onClick={onDone}>
          Отмена
        </button>
      </div>
    </div>
  );
}

/** Новый долг или правка. Для «я должен» — можно сразу завести график платежей (кредит, рассрочка). */
function DebtEditor({ snap, view, act, onClose }: { snap: Snapshot; view: DebtView | null; act: FinanceActions; onClose: () => void }) {
  const d = view?.debt;
  const [dir, setDir] = useState<DebtDir>(d?.dir ?? 'i_owe');
  const [who, setWho] = useState(d?.who ?? '');
  const [amount, setAmount] = useState(d ? amountText(d.amount) : '');
  const [date, setDate] = useState(d?.date ?? snap.today);
  const [due, setDue] = useState(d?.due ?? '');
  const [note, setNote] = useState(d?.note ?? '');
  const [money, setMoney] = useState(false);
  const [account, setAccount] = useState(accountByKind(snap, 'personal'));
  const [credit, setCredit] = useState(false);
  const [pay, setPay] = useState('');
  const [first, setFirst] = useState(addMonths(snap.today, 1));
  const [auto, setAuto] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const iOwe = dir === 'i_owe';
  const save = async () => {
    const k = parseAmount(amount);
    if (!who.trim()) return setError(iOwe ? 'Укажите, кому вы должны' : 'Укажите, кто вам должен');
    if (!k || k <= 0) return setError('Введите сумму долга');
    const payK = credit ? parseAmount(pay) : null;
    if (credit && (!payK || payK <= 0)) return setError('Введите сумму ежемесячного платежа');
    const debt: Debt = { ...(d ?? { closed: false }), id: d?.id ?? 'debt-' + newId(), who: who.trim(), dir, amount: k, date: date || snap.today, due: due || null, note: note.trim(), closed: d?.closed ?? false };
    const s = await act.run(api.debtSave(debt, !d && money ? account : null), d ? 'Долг сохранён' : 'Долг записан');
    if (!s) return;
    if (!d && credit && iOwe && payK) {
      const plan: Plan = { id: newId(), title: 'Платёж: ' + debt.who, amount: payK, account, cat: '', every: 'month', start: first || snap.today, until: null, auto, remind: null, debt: debt.id, paused: false, note: '' };
      if (!(await act.run(api.planSave(plan), 'Долг и график платежей записаны'))) return;
    }
    if (debt.due || credit) act.saved();
    onClose();
  };
  return (
    <div className="card stack gap12">
      <h2 className="h2">{d ? 'Долг' : 'Новый долг'}</h2>
      <Seg
        label="Кто кому должен"
        value={dir}
        onPick={setDir}
        items={[
          { id: 'i_owe', label: 'Я должен' },
          { id: 'owed_to_me', label: 'Мне должны' },
        ]}
      />
      <div className="form-grid">
        <label className="label">
          <span>{iOwe ? 'Кому' : 'Кто'}</span>
          <input className="field" value={who} onChange={(e) => setWho(e.target.value)} placeholder={iOwe ? 'Например, Сбербанк или Иван' : 'Например, Иван'} />
        </label>
        <label className="label">
          <span>{iOwe ? 'Сколько осталось вернуть, ₽' : 'Сколько должны, ₽'}</span>
          <input className="field num" inputMode="decimal" value={amount} onChange={(e) => setAmount(e.target.value)} placeholder="0" />
        </label>
        <label className="label">
          <span>Когда появился</span>
          <input className="field" type="date" value={date} onChange={(e) => setDate(e.target.value)} />
        </label>
        <label className="label">
          <span>Вернуть до (необязательно)</span>
          <input className="field" type="date" value={due} onChange={(e) => setDue(e.target.value)} />
        </label>
      </div>
      <label className="label">
        <span>Заметка</span>
        <input className="field" value={note} onChange={(e) => setNote(e.target.value)} placeholder="Например, за ремонт машины" />
      </label>
      {!d && (
        <>
          <Check checked={money} onChange={setMoney}>
            {iOwe ? 'Деньги пришли на счёт — записать' : 'Деньги ушли со счёта — записать'}
          </Check>
          {money && <AccountChips snap={snap} value={account} onPick={setAccount} label="Счёт" />}
          <p className="note" style={{ marginTop: -6 }}>
            Сам долг «Мои деньги» не уменьшает: меняются только остатки на счетах, когда деньги действительно двигаются.
          </p>
        </>
      )}
      {!d && iOwe && (
        <>
          <Check checked={credit} onChange={setCredit}>
            Кредит или рассрочка: платить по графику
          </Check>
          {credit && (
            <div className="stack gap10">
              <div className="form-grid">
                <label className="label">
                  <span>Платёж в месяц, ₽</span>
                  <input className="field num" inputMode="decimal" value={pay} onChange={(e) => setPay(e.target.value)} placeholder="0" />
                </label>
                <label className="label">
                  <span>Ближайший платёж</span>
                  <input className="field" type="date" value={first} onChange={(e) => setFirst(e.target.value)} />
                </label>
              </div>
              <span className="note">С какого счёта платите</span>
              <AccountChips snap={snap} value={account} onPick={setAccount} label="Счёт платежа" />
              <Check checked={auto} onChange={setAuto}>
                Списывается само (автоплатёж)
              </Check>
            </div>
          )}
        </>
      )}
      <ErrorText text={error} />
      <div className="wrap">
        <button type="button" className="btn own" onClick={save}>
          {d ? 'Сохранить' : 'Записать долг'}
        </button>
        <button type="button" className="btn ghost" onClick={onClose}>
          Отмена
        </button>
      </div>
    </div>
  );
}

/** Новый регулярный платёж или правка. */
function PlanEditor({ snap, view, act, onClose }: { snap: Snapshot; view: PlanView | null; act: FinanceActions; onClose: () => void }) {
  const p = view?.plan;
  const [title, setTitle] = useState(p?.title ?? '');
  const [amount, setAmount] = useState(p ? amountText(p.amount) : '');
  const [account, setAccount] = useState(p?.account ?? accountByKind(snap, 'personal'));
  const [cat, setCat] = useState(p?.cat || (OWN_OUT.includes('telecom') ? 'telecom' : OWN_OUT[0]));
  const [allCats, setAllCats] = useState(false);
  // Платёж со скрытой категорией: её кнопка видна, пока категория выбрана.
  const catsAll = !OWN_OUT.includes(cat) && OWN_OUT_ALL.includes(cat) ? [...OWN_OUT, cat] : OWN_OUT;
  const catsShown = allCats ? catsAll : topCats(catsAll, snap.ops, cat);
  const [every, setEvery] = useState<Every>(p?.every ?? 'month');
  const [start, setStart] = useState(p?.start ?? snap.today);
  const [until, setUntil] = useState(p?.until ?? '');
  const [auto, setAuto] = useState(p?.auto ?? false);
  const [remind, setRemind] = useState(p?.remind == null ? '' : String(p.remind));
  const [debt, setDebt] = useState(p?.debt ?? '');
  const [paused, setPaused] = useState(p?.paused ?? false);
  const [error, setError] = useState<string | null>(null);
  const cur = snap.accounts.find((a) => a.account.id === account)?.account.currency ?? 'RUB';
  const credits = snap.debts.filter((v) => v.debt.dir === 'i_owe' && (!v.debt.closed || v.debt.id === debt));
  const save = async () => {
    const k = parseAmount(amount);
    if (!title.trim()) return setError('Укажите название платежа');
    if (!k || k <= 0) return setError('Введите сумму платежа');
    if (until && until < start) return setError('Последний платёж раньше первого');
    const plan: Plan = { ...(p ?? { skipped: [] }), id: p?.id ?? newId(), title: title.trim(), amount: k, account, cat: debt ? '' : cat, every, start: start || snap.today, until: until || null, auto, remind: remind === '' ? null : Number(remind), debt: debt || null, paused, note: p?.note ?? '' };
    const s = await act.run(api.planSave(plan), p ? 'Платёж сохранён' : 'Регулярный платёж записан');
    if (!s) return;
    act.saved();
    onClose();
  };
  return (
    <div className="card stack gap12">
      <h2 className="h2">{p ? 'Регулярный платёж' : 'Новый регулярный платёж'}</h2>
      <div className="form-grid">
        <label className="label">
          <span>Название</span>
          <input className="field" value={title} onChange={(e) => setTitle(e.target.value)} placeholder="Например, Интернет" />
        </label>
        <label className="label">
          <span>Сумма, {SIGN[cur] || cur}</span>
          <input className="field num" inputMode="decimal" value={amount} onChange={(e) => setAmount(e.target.value)} placeholder="0" />
        </label>
      </div>
      <span className="note">Счёт</span>
      <AccountChips snap={snap} value={account} onPick={setAccount} label="Счёт" />
      <Seg
        label="Как часто"
        value={every}
        onPick={setEvery}
        items={[
          { id: 'month', label: 'Каждый месяц' },
          { id: 'week', label: 'Неделя' },
          { id: 'year', label: 'Год' },
        ]}
      />
      <div className="form-grid">
        <label className="label">
          <span>{p ? 'Первый платёж' : 'Ближайший платёж'}</span>
          <input className="field" type="date" value={start} onChange={(e) => setStart(e.target.value)} />
        </label>
        <label className="label">
          <span>Последний (необязательно)</span>
          <input className="field" type="date" value={until} onChange={(e) => setUntil(e.target.value)} />
        </label>
      </div>
      <p className="note" style={{ marginTop: -6 }}>
        {start ? 'Повтор: ' + everyText({ ...(p as Plan), every, start }) + '.' : ''}
      </p>
      {credits.length > 0 && (
        <label className="label">
          <span>Кредит или рассрочка</span>
          <select className="field" value={debt} onChange={(e) => setDebt(e.target.value)}>
            <option value="">Нет — обычный расход</option>
            {credits.map((v) => (
              <option key={v.debt.id} value={v.debt.id}>
                {v.debt.who}, осталось {rub(v.left)}
              </option>
            ))}
          </select>
        </label>
      )}
      {!debt && (
        <div className={'chips' + (allCats ? '' : ' scroll')} role="group" aria-label="Категория">
          {catsShown.map((c) => (
            <button key={c} type="button" aria-pressed={c === cat} className={'chip' + (c === cat ? ' sel' : '')} onClick={() => setCat(c)}>
              {(CATS[c] || CATS.other).s}
            </button>
          ))}
          {catsAll.length > catsShown.length && (
            <button type="button" className="chip more" aria-label={`Ещё категории: ${catsAll.length - catsShown.length}`} onClick={() => setAllCats(true)}>
              Ещё {catsAll.length - catsShown.length}
            </button>
          )}
        </div>
      )}
      <label className="label">
        <span>Напоминать</span>
        <select className="field" value={remind} onChange={(e) => setRemind(e.target.value)}>
          {REMIND.map((r) => (
            <option key={r.id} value={r.id}>
              {r.id === '' ? `Как в настройках (${REMIND.find((x) => x.id === String(snap.remind_days))?.label.toLowerCase() ?? 'за ' + snap.remind_days + ' дн.'})` : r.label}
            </option>
          ))}
        </select>
      </label>
      <Check checked={auto} onChange={setAuto}>
        Списывается само (автоплатёж с карты)
      </Check>
      <p className="note" style={{ marginTop: -6 }}>
        {auto ? 'В день платежа операция запишется сама. Если списания не было — удалите её или нажмите «Пропустить».' : 'В день платежа «Карман» напомнит, а вы нажмёте «Оплачено».'}
      </p>
      {p && (
        <Check checked={paused} onChange={setPaused}>
          Приостановить: не напоминать и не списывать
        </Check>
      )}
      <ErrorText text={error} />
      <div className="wrap">
        <button type="button" className="btn own" onClick={save}>
          {p ? 'Сохранить' : 'Записать платёж'}
        </button>
        <button type="button" className="btn ghost" onClick={onClose}>
          Отмена
        </button>
      </div>
    </div>
  );
}

function DebtRow({ snap, view, act, onEdit }: { snap: Snapshot; view: DebtView; act: FinanceActions; onEdit: () => void }) {
  const [open, setOpen] = useState(false);
  const [paying, setPaying] = useState(false);
  const d = view.debt;
  const iOwe = d.dir === 'i_owe';
  const plan = view.plan ? snap.plans.find((p) => p.plan.id === view.plan) : undefined;
  const history = snap.ops.filter((o) => o.kind === 'debt' && o.debt === d.id);
  let sub = iOwe ? 'я должен' : 'мне должны';
  if (d.closed) sub += ', закрыт';
  else if (view.left === 0) sub += ', возвращён';
  else if (view.overdue && d.due) sub += ', срок прошёл ' + dMon(d.due);
  else if (d.due) sub += ', до ' + dMon(d.due);
  if (plan && view.next && view.left > 0) sub += ', платёж ' + dMon(view.next);
  return (
    <div className="op">
      <button type="button" aria-expanded={open} onClick={() => setOpen(!open)}>
        <span className="opicon own">
          <Icon name="debt" size={19} />
        </span>
        <span className="stack" style={{ minWidth: 0 }}>
          <span className="optitle">{d.who}</span>
          <span className={'opsub' + (view.overdue ? ' warn' : '')}>{sub}</span>
        </span>
        <span className="opright">
          <span className={'amount' + (iOwe ? '' : ' in')}>{rub(d.closed ? 0 : view.left)}</span>
          {view.repaid > 0 && view.left > 0 && <span className="note faint">из {rub(d.amount)}</span>}
        </span>
      </button>
      {open && (
        <div className="opdetails">
          <div className="d">
            <span>{iOwe ? 'Нужно было вернуть' : 'Должны были'}</span>
            <span>{rub(d.amount)}</span>
          </div>
          <div className="d">
            <span>Возвращено</span>
            <span>{rub(view.repaid)}</span>
          </div>
          {plan && (
            <div className="d">
              <span>По графику</span>
              <span style={{ textAlign: 'right' }}>
                {rub(plan.plan.amount, snap.accounts.find((a) => a.account.id === plan.plan.account)?.account.currency)} {everyText(plan.plan)}
                {plan.plan.auto ? ', списывается само' : ''}
              </span>
            </div>
          )}
          {d.note && (
            <div className="d">
              <span>Заметка</span>
              <span style={{ textAlign: 'right' }}>{d.note}</span>
            </div>
          )}
          {history.length > 0 && (
            <div className="stack" style={{ gap: 2 }}>
              {history.slice(0, 6).map((o) => {
                const v = opView(snap, o);
                return (
                  <div className="d" key={o.id}>
                    <span>
                      {dMon(o.date)}, {v.title}
                    </span>
                    <span className={'num' + (v.inc ? ' pos' : '')}>{v.amount}</span>
                  </div>
                );
              })}
            </div>
          )}
          {paying ? (
            <DebtPay snap={snap} view={view} act={act} onDone={() => setPaying(false)} />
          ) : (
            <div className="wrap">
              {!d.closed && view.left > 0 && (
                <button type="button" className="btn own" onClick={() => setPaying(true)}>
                  Записать возврат
                </button>
              )}
              <button type="button" className="btn ghost" onClick={onEdit}>
                Изменить
              </button>
              <button type="button" className="btn ghost" onClick={() => act.run(api.debtSave({ ...d, closed: !d.closed }, null), d.closed ? 'Долг открыт снова' : 'Долг закрыт')}>
                {d.closed ? 'Открыть снова' : 'Закрыть долг'}
              </button>
              <button
                type="button"
                className="btn link"
                onClick={() => act.run(api.debtDelete(d.id), view.plan ? 'Долг и его график удалены, записанные платежи остались в ленте' : 'Долг удалён, записанные платежи остались в ленте')}
              >
                Удалить
              </button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function PlanRow({ snap, view, act, onEdit }: { snap: Snapshot; view: PlanView; act: FinanceActions; onEdit: () => void }) {
  const [open, setOpen] = useState(false);
  const p = view.plan;
  const cur = snap.accounts.find((a) => a.account.id === p.account)?.account.currency ?? 'RUB';
  const acc = snap.accounts.find((a) => a.account.id === p.account)?.account.name ?? 'Счёт удалён';
  const late = !!view.next && view.next < snap.today;
  let sub = everyText(p);
  if (p.paused) sub += ', приостановлен';
  else if (view.next) sub += (late ? ', просрочен ' : ', следующий ') + dMon(view.next);
  else sub += ', все платежи сделаны';
  return (
    <div className="op">
      <button type="button" aria-expanded={open} onClick={() => setOpen(!open)}>
        <span className="opicon own">
          <Icon name={p.debt ? 'credit' : (CATS[p.cat] || CATS.other).i} size={19} />
        </span>
        <span className="stack" style={{ minWidth: 0 }}>
          <span className="optitle">{p.title}</span>
          <span className={'opsub' + (late ? ' warn' : '')}>{sub}</span>
        </span>
        <span className="opright">
          <span className="amount">{rub(p.amount, cur)}</span>
          {p.auto && <span className="tag grey">само</span>}
        </span>
      </button>
      {open && (
        <div className="opdetails">
          <div className="d">
            <span>Счёт</span>
            <span>{acc}</span>
          </div>
          {p.until && (
            <div className="d">
              <span>Последний платёж</span>
              <span>{dMon(p.until)}</span>
            </div>
          )}
          <div className="d">
            <span>Оплачено платежей</span>
            <span>{view.paid}</span>
          </div>
          {view.overdue > 1 && (
            <div className="d">
              <span>Просрочено</span>
              <span className="warn">{view.overdue}</span>
            </div>
          )}
          <div className="wrap">
            {view.next && !p.paused && (
              <button type="button" className="btn own" onClick={() => act.run(api.planPay(p.id, view.next as string, null, null), `Оплачено: ${p.title}, ${dMon(view.next as string)}`)}>
                Оплачено за {dMon(view.next)}
              </button>
            )}
            <button type="button" className="btn ghost" onClick={onEdit}>
              Изменить
            </button>
            <button type="button" className="btn link" onClick={() => act.run(api.planDelete(p.id), 'Платёж удалён, записанные операции остались в ленте')}>
              Удалить
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

export default function Finance({ snap, act, view, setView, goSettings, openTrip, mobile }: { snap: Snapshot; act: FinanceActions; view: FinView; setView: (v: FinView) => void; goSettings: () => void; openTrip: (id: string) => void; mobile: boolean }) {
  return (
    <div className="stack gap16">
      <h1 className="h1">Финансы</h1>
      <Seg
        label="Раздел финансов"
        fit
        value={view}
        onPick={setView}
        items={[
          { id: 'pay', label: 'Платежи' },
          { id: 'budget', label: 'Бюджет' },
          { id: 'save', label: 'Накопления' },
          { id: 'stats', label: 'Статистика' },
        ]}
      />
      {view === 'pay' && <Payments snap={snap} act={act} />}
      {view === 'budget' && <BudgetScreen snap={snap} act={act} />}
      {view === 'save' && <Savings snap={snap} act={act} goSettings={goSettings} />}
      {view === 'stats' && <StatsScreen snap={snap} act={act} mobile={mobile} openTrip={openTrip} />}
    </div>
  );
}

/** Платежи по графику, сроки и долги. */
function Payments({ snap, act }: { snap: Snapshot; act: FinanceActions }) {
  const [editDebt, setEditDebt] = useState<string | null>(null);
  const [editPlan, setEditPlan] = useState<string | null>(null);
  const [allDue, setAllDue] = useState(false);
  const [showClosed, setShowClosed] = useState(false);
  const due = snap.upcoming;
  const shown = allDue ? due : due.filter((u) => u.soon).slice(0, 8);
  const openDebts = snap.debts.filter(isOpen);
  const closedDebts = snap.debts.filter((v) => !isOpen(v));
  const debtView = editDebt && editDebt !== 'new' ? snap.debts.find((v) => v.debt.id === editDebt) ?? null : null;
  const planView = editPlan && editPlan !== 'new' ? snap.plans.find((v) => v.plan.id === editPlan) ?? null : null;
  return (
    <div className="stack gap26" style={{ marginTop: 10 }}>
      <section className="stack">
        <div className="between" style={{ marginBottom: 8 }}>
          <h2 className="h2">Скоро</h2>
          {due.length > shown.length && (
            <button type="button" className="btn link" style={{ color: 'var(--own)', padding: 0 }} onClick={() => setAllDue(true)}>
              Весь месяц: {due.length}
            </button>
          )}
        </div>
        {shown.length === 0 && <p className="empty">{due.length ? `Ближайший платёж — ${dMon(due[0].date)}: ${due[0].title}.` : 'Платежей и сроков в ближайший месяц нет.'}</p>}
        {shown.map((u) => (
          <DueRow key={u.kind + u.id + u.date} snap={snap} due={u} act={act} />
        ))}
      </section>

      <section className="stack">
        <h2 className="h2" style={{ marginBottom: 8 }}>
          Долги
        </h2>
        {(snap.i_owe > 0 || snap.owed_to_me > 0) && (
          <div className="figures" style={{ marginBottom: 6 }}>
            <div className="figure" style={{ cursor: 'default' }}>
              <span className="lab">Я должен</span>
              <span className="val" style={{ fontSize: 22, lineHeight: '28px' }}>
                {rub(snap.i_owe)}
              </span>
            </div>
            <div className="figure" style={{ cursor: 'default' }}>
              <span className="lab">Мне должны</span>
              <span className="val" style={{ fontSize: 22, lineHeight: '28px' }}>
                {rub(snap.owed_to_me)}
              </span>
            </div>
          </div>
        )}
        {editDebt === 'new' && <DebtEditor snap={snap} view={null} act={act} onClose={() => setEditDebt(null)} />}
        {openDebts.map((v) =>
          editDebt === v.debt.id && debtView ? (
            <DebtEditor key={v.debt.id} snap={snap} view={debtView} act={act} onClose={() => setEditDebt(null)} />
          ) : (
            <DebtRow key={v.debt.id} snap={snap} view={v} act={act} onEdit={() => setEditDebt(v.debt.id)} />
          ),
        )}
        {openDebts.length === 0 && editDebt !== 'new' && <p className="empty">Долгов нет. Кредит или рассрочку тоже можно записать как долг — с графиком платежей.</p>}
        {closedDebts.length > 0 && (
          <button type="button" className="collapse" aria-expanded={showClosed} onClick={() => setShowClosed(!showClosed)}>
            <span>Возвращённые и закрытые: {closedDebts.length}</span>
            <Icon name={showClosed ? 'up' : 'down'} size={18} stroke={2} />
          </button>
        )}
        {showClosed &&
          closedDebts.map((v) =>
            editDebt === v.debt.id && debtView ? (
              <DebtEditor key={v.debt.id} snap={snap} view={debtView} act={act} onClose={() => setEditDebt(null)} />
            ) : (
              <DebtRow key={v.debt.id} snap={snap} view={v} act={act} onEdit={() => setEditDebt(v.debt.id)} />
            ),
          )}
        {editDebt !== 'new' && (
          <button type="button" className="btn ghost" style={{ marginTop: 10, alignSelf: 'flex-start' }} onClick={() => setEditDebt('new')}>
            Новый долг
          </button>
        )}
      </section>

      <section className="stack">
        <h2 className="h2" style={{ marginBottom: 8 }}>
          Регулярные платежи
        </h2>
        {editPlan === 'new' && <PlanEditor snap={snap} view={null} act={act} onClose={() => setEditPlan(null)} />}
        {snap.plans.map((v) =>
          editPlan === v.plan.id && planView ? (
            <PlanEditor key={v.plan.id} snap={snap} view={planView} act={act} onClose={() => setEditPlan(null)} />
          ) : (
            <PlanRow key={v.plan.id} snap={snap} view={v} act={act} onEdit={() => setEditPlan(v.plan.id)} />
          ),
        )}
        {snap.plans.length === 0 && editPlan !== 'new' && <p className="empty">Связь, аренда, подписки, коммуналка — «Карман» напомнит заранее, а автоплатежи запишет сам.</p>}
        {editPlan !== 'new' && (
          <button type="button" className="btn ghost" style={{ marginTop: 10, alignSelf: 'flex-start' }} onClick={() => setEditPlan('new')}>
            Новый платёж
          </button>
        )}
        <p className="note" style={{ marginTop: 10 }}>
          Напоминания: {snap.remind_days ? `за ${snap.remind_days}${NB}дн. и в день платежа` : 'в день платежа'} — меняется в настройках.
        </p>
      </section>
    </div>
  );
}
