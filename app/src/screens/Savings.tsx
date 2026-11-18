import { useState } from 'react';
import { api, type Goal, type GoalView, type Snapshot } from '../api';
import { NB, amountText, dMon, parseAmount, plural, rateText, rub } from '../format';
import { ErrorText, Icon } from '../ui';
import { Bar } from './Budget';
import type { FinanceActions } from './Finance';

export const SAVING = new Set(['savings', 'deposit']);

function newId(): string {
  const b = new Uint8Array(8);
  crypto.getRandomValues(b);
  return 'goal-' + Array.from(b, (x) => x.toString(16).padStart(2, '0')).join('');
}

const rubAccounts = (snap: Snapshot) => snap.accounts.filter((a) => !a.account.archived && a.account.currency === 'RUB');

/** «Отложить» на цель или «Взять из цели»: сумма, другой счёт, дата. */
function GoalMove({ snap, view, take, act, onDone }: { snap: Snapshot; view: GoalView; take: boolean; act: FinanceActions; onDone: () => void }) {
  const others = rubAccounts(snap).filter((a) => a.account.id !== view.goal.account);
  const [amount, setAmount] = useState(take ? amountText(view.saved) : view.per_month ? amountText(view.per_month) : '');
  const [account, setAccount] = useState((others.find((a) => a.account.kind === 'personal') || others[0])?.account.id ?? '');
  const [date, setDate] = useState(snap.today);
  const [error, setError] = useState<string | null>(null);
  const save = async () => {
    const k = parseAmount(amount);
    if (!k || k <= 0) return setError('Введите сумму');
    if (!account) return setError('Нужен ещё один рублёвый счёт');
    if (take && k > view.saved) return setError('На цели отложено только ' + rub(view.saved));
    const s = await act.run(api.goalMove(view.goal.id, k, account, date || snap.today, take), (take ? 'Взято из цели: ' : 'Отложено на цель: ') + rub(k));
    if (s) onDone();
  };
  return (
    <div className="stack gap10" style={{ paddingTop: 6 }}>
      <div className="form-grid">
        <label className="field">
          <span>Сумма, ₽</span>
          <input className="input num" inputMode="decimal" value={amount} onChange={(e) => setAmount(e.target.value)} autoFocus />
        </label>
        <label className="field">
          <span>Дата</span>
          <input className="input" type="date" value={date} onChange={(e) => setDate(e.target.value)} />
        </label>
      </div>
      <span className="small muted">{take ? 'На какой счёт' : 'С какого счёта'}</span>
      <div className="chips scroll" role="group" aria-label={take ? 'На какой счёт' : 'С какого счёта'}>
        {others.map((a) => (
          <button key={a.account.id} type="button" aria-pressed={a.account.id === account} className={'chip' + (a.account.id === account ? ' sel' : '')} style={{ minWidth: 44, padding: '0 10px' }} onClick={() => setAccount(a.account.id)}>
            {a.account.short || a.account.name}
          </button>
        ))}
      </div>
      <ErrorText text={error} />
      <div className="sec-actions">
        <button type="button" className="btn own" onClick={save}>
          {take ? 'Взять' : 'Отложить'}
        </button>
        <button type="button" className="btn line" onClick={onDone}>
          Отмена
        </button>
      </div>
    </div>
  );
}

function GoalEditor({ snap, view, act, onClose }: { snap: Snapshot; view: GoalView | null; act: FinanceActions; onClose: () => void }) {
  const g = view?.goal;
  const saving = snap.accounts.filter((a) => !a.account.archived && SAVING.has(a.account.kind) && a.account.currency === 'RUB');
  const choice = saving.length ? saving : rubAccounts(snap);
  const [title, setTitle] = useState(g?.title ?? '');
  const [target, setTarget] = useState(g ? amountText(g.target) : '');
  const [due, setDue] = useState(g?.due ?? '');
  const [account, setAccount] = useState(g?.account ?? choice[0]?.account.id ?? '');
  const [error, setError] = useState<string | null>(null);
  const save = async () => {
    const k = parseAmount(target);
    if (!title.trim()) return setError('Укажите, на что копите');
    if (!k || k <= 0) return setError('Введите, сколько нужно');
    if (!account) return setError('Выберите счёт');
    const goal: Goal = { ...(g ?? { done: false, note: '' }), id: g?.id ?? newId(), title: title.trim(), target: k, due: due || null, account, done: g?.done ?? false, note: g?.note ?? '' };
    if (await act.run(api.goalSave(goal), g ? 'Цель сохранена' : 'Цель записана')) onClose();
  };
  return (
    <div className="card stack gap12">
      <h2 className="sec">{g ? 'Цель' : 'Новая цель'}</h2>
      <div className="form-grid">
        <label className="field">
          <span>На что копите</span>
          <input className="input" value={title} onChange={(e) => setTitle(e.target.value)} placeholder="Например, отпуск" />
        </label>
        <label className="field">
          <span>Сколько нужно, ₽</span>
          <input className="input num" inputMode="decimal" value={target} onChange={(e) => setTarget(e.target.value)} placeholder="0" />
        </label>
        <label className="field">
          <span>К какой дате (необязательно)</span>
          <input className="input" type="date" value={due} onChange={(e) => setDue(e.target.value)} />
        </label>
      </div>
      <span className="small muted">Куда откладывать</span>
      <div className="chips scroll" role="group" aria-label="Счёт цели">
        {choice.map((a) => (
          <button key={a.account.id} type="button" aria-pressed={a.account.id === account} className={'chip' + (a.account.id === account ? ' sel' : '')} style={{ minWidth: 44, padding: '0 10px' }} onClick={() => setAccount(a.account.id)}>
            {a.account.short || a.account.name}
          </button>
        ))}
      </div>
      {!saving.length && <p className="small warn-text">Накопительного счёта нет. Заведите его в настройках (тип «Накопительный»), чтобы цель шла в «Отложено».</p>}
      <p className="small muted" style={{ marginTop: -6 }}>
        «Отложить» — перевод на этот счёт с меткой цели. На одном счёте можно копить на несколько целей.
      </p>
      <ErrorText text={error} />
      <div className="sec-actions">
        <button type="button" className="btn own" onClick={save}>
          {g ? 'Сохранить' : 'Записать цель'}
        </button>
        <button type="button" className="btn line" onClick={onClose}>
          Отмена
        </button>
      </div>
    </div>
  );
}

function GoalRow({ snap, view, act, onEdit }: { snap: Snapshot; view: GoalView; act: FinanceActions; onEdit: () => void }) {
  const [open, setOpen] = useState(false);
  const [move, setMove] = useState<'put' | 'take' | null>(null);
  const g = view.goal;
  const acc = snap.accounts.find((a) => a.account.id === g.account)?.account.name ?? 'счёт удалён';
  let sub = `${rub(view.saved)} из ${rub(g.target)}`;
  if (g.done) sub += ', достигнута';
  else if (view.left === 0) sub += ', собрано';
  else if (g.due && view.per_month) sub += `, до ${dMon(g.due)} — по ${rub(view.per_month)} в месяц`;
  return (
    <div className="op">
      <button type="button" aria-expanded={open} onClick={() => setOpen(!open)}>
        <span className="opicon own">
          <Icon name="interest" size={18} />
        </span>
        <span className="stack gap0" style={{ minWidth: 0, gap: 4 }}>
          <span className="optitle">{g.title}</span>
          <Bar value={view.saved} limit={g.target} />
          <span className="opsub">{sub}</span>
        </span>
        <span className="opright">
          <span className="amount">{Math.min(100, Math.round((view.saved / g.target) * 100))}%</span>
        </span>
      </button>
      {open && (
        <div className="opdetails">
          <div className="d">
            <span>Счёт</span>
            <span>{acc}</span>
          </div>
          <div className="d">
            <span>Осталось собрать</span>
            <span>{rub(view.left)}</span>
          </div>
          {g.due && view.months_left != null && (
            <div className="d">
              <span>До срока</span>
              <span>
                {view.months_left}
                {NB}
                {plural(view.months_left, 'месяц', 'месяца', 'месяцев')}, до {dMon(g.due)}
              </span>
            </div>
          )}
          {move ? (
            <GoalMove snap={snap} view={view} take={move === 'take'} act={act} onDone={() => setMove(null)} />
          ) : (
            <div className="sec-actions">
              {!g.done && (
                <button type="button" className="btn own" onClick={() => setMove('put')}>
                  Отложить
                </button>
              )}
              {view.saved > 0 && (
                <button type="button" className="btn line" onClick={() => setMove('take')}>
                  Взять из цели
                </button>
              )}
              <button type="button" className="btn line" onClick={onEdit}>
                Изменить
              </button>
              <button type="button" className="btn line" onClick={() => act.run(api.goalSave({ ...g, done: !g.done }), g.done ? 'Цель снова в работе' : 'Цель достигнута!')}>
                {g.done ? 'Вернуть в работу' : 'Цель достигнута'}
              </button>
              <button type="button" className="link-btn warn-text" onClick={() => act.run(api.goalDelete(g.id), 'Цель удалена, переводы остались в ленте')}>
                Удалить
              </button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

export default function Savings({ snap, act, goSettings }: { snap: Snapshot; act: FinanceActions; goSettings: () => void }) {
  const [edit, setEdit] = useState<string | null>(null);
  const accounts = snap.accounts.filter((a) => !a.account.archived && SAVING.has(a.account.kind));
  const goalView = edit && edit !== 'new' ? snap.goals.find((v) => v.goal.id === edit) ?? null : null;
  return (
    <div className="stack gap26">
      <section className="stack gap0">
        <div className="between" style={{ marginBottom: 8 }}>
          <h2 className="sec">Отложено</h2>
          <span className="num" style={{ fontWeight: 500 }}>
            {rub(snap.saved)}
          </span>
        </div>
        <p className="small muted" style={{ marginBottom: 6 }}>
          {snap.saved_in_mine ? 'Входит в «Мои деньги» — так задано в настройках.' : 'Не входит в «Мои деньги»: это деньги на потом. Включить — в настройках, раздел «Счета».'}
        </p>
        {accounts.map((a) => {
          const d = a.account.deposit;
          const f = snap.deposits.find((x) => x.account === a.account.id);
          return (
            <div key={a.account.id} className="budrow">
              <span className="opicon own">
                <Icon name={a.account.kind === 'deposit' ? 'interest' : 'finance'} size={18} />
              </span>
              <span className="stack gap0" style={{ minWidth: 0, gap: 2 }}>
                <span className="between">
                  <span>{a.account.name}</span>
                  <span className="num">{rub(a.balance, a.account.currency)}</span>
                </span>
                {d && f && (
                  <span className="small muted">
                    {rateText(d.rate)}% годовых{d.capitalize ? ', с капитализацией' : ''} · ≈{NB}+{rub(f.month)} в месяц
                    {f.until && f.days_left != null && f.days_left >= 0 ? ` · до ${dMon(f.until)}: доход ≈${NB}${rub(f.income)}, будет ≈${NB}${rub(f.at_end)}` : f.until ? ` · срок закончился ${dMon(f.until)}` : ''}
                  </span>
                )}
                {a.account.kind === 'deposit' && !d && <span className="small muted">Условия вклада не заданы — укажите ставку и срок в настройках.</span>}
              </span>
            </div>
          );
        })}
        {accounts.length === 0 && <p className="row muted">Накопительных счетов и вкладов нет. Заведите их в настройках: тип счёта «Накопительный» или «Вклад».</p>}
        <button type="button" className="link-btn" style={{ alignSelf: 'flex-start' }} onClick={goSettings}>
          Счета и вклады — в настройках
        </button>
      </section>

      <section className="stack gap0">
        <h2 className="sec" style={{ marginBottom: 8 }}>
          Цели
        </h2>
        {edit === 'new' && <GoalEditor snap={snap} view={null} act={act} onClose={() => setEdit(null)} />}
        {snap.goals.map((v) =>
          edit === v.goal.id && goalView ? (
            <GoalEditor key={v.goal.id} snap={snap} view={goalView} act={act} onClose={() => setEdit(null)} />
          ) : (
            <GoalRow key={v.goal.id} snap={snap} view={v} act={act} onEdit={() => setEdit(v.goal.id)} />
          ),
        )}
        {snap.goals.length === 0 && edit !== 'new' && <p className="row muted">Отпуск, ремонт, подушка безопасности — «Карман» посчитает, сколько откладывать в месяц, чтобы успеть.</p>}
        {edit !== 'new' && (
          <button type="button" className="btn line" style={{ marginTop: 10, alignSelf: 'flex-start' }} onClick={() => setEdit('new')}>
            Новая цель
          </button>
        )}
      </section>
    </div>
  );
}
