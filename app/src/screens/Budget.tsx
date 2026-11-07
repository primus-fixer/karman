import { useState } from 'react';
import { api, type Budget, type BudgetView, type Snapshot } from '../api';
import { NB, amountText, dRange, parseAmount, plural, rub } from '../format';
import { CATS, OWN_OUT } from '../logic';
import { ErrorText, Icon, Seg } from '../ui';
import type { FinanceActions } from './Finance';

/** Полоска «потрачено из лимита»: красная, если лимит превышен. */
export function Bar({ value, limit }: { value: number; limit: number | null }) {
  if (!limit) return null;
  const share = Math.min(1, value / limit);
  const over = value > limit;
  return (
    <span className={'bar' + (over ? ' over' : share >= 0.85 ? ' near' : '')} role="img" aria-label={over ? 'сверх лимита' : `${Math.round(share * 100)}% лимита`}>
      <i style={{ width: `${Math.round(share * 100)}%` }} />
    </span>
  );
}

/** «осталось 2 100 ₽» или «сверх лимита на 900 ₽». */
export function leftText(spent: number, limit: number): string {
  return spent <= limit ? 'осталось ' + rub(limit - spent) : 'сверх лимита на ' + rub(spent - limit);
}

const NAMES: Record<string, string> = { personal: 'Личный', family: 'Семейный' };

/** Бюджет одного вида: личный или семейный. */
function BudgetCard({ snap, view, act }: { snap: Snapshot; view: BudgetView; act: FinanceActions }) {
  const [edit, setEdit] = useState(false);
  const limited = view.cats.filter((c) => c.limit != null);
  const free = view.cats.filter((c) => c.limit == null);
  const order = (a: string, b: string) => OWN_OUT.indexOf(a) - OWN_OUT.indexOf(b);
  const perDay = view.total && view.total > view.spent && view.days_left > 0 ? Math.floor((view.total - view.spent) / view.days_left / 100) * 100 : 0;
  if (edit) return <BudgetEditor snap={snap} view={view} act={act} onClose={() => setEdit(false)} />;
  const empty = !view.total && limited.length === 0;
  return (
    <div className="stack gap10">
      <p className="note">
        {dRange(view.start, view.end)}, {view.days_left > 0 ? `осталось ${view.days_left}${NB}${plural(view.days_left, 'день', 'дня', 'дней')}` : 'последний день'}
      </p>
      <div className="stack" style={{ gap: 6 }}>
        <div className="between">
          <span style={{ fontWeight: 500 }}>Всего</span>
          <span className="num">
            {rub(view.spent)}
            {view.total ? ` из ${rub(view.total)}` : ''}
          </span>
        </div>
        <Bar value={view.spent} limit={view.total} />
        {view.total ? (
          <span className={'note' + (view.spent > view.total ? ' warn' : '')}>
            {leftText(view.spent, view.total)}
            {perDay > 0 ? `, ≈${NB}${rub(perDay)} в день` : ''}
          </span>
        ) : null}
      </div>
      {limited
        .slice()
        .sort((a, b) => order(a.cat, b.cat))
        .map((c) => (
          <div key={c.cat} className="budrow">
            <span className="opicon own">
              <Icon name={(CATS[c.cat] || CATS.other).i} size={18} />
            </span>
            <span className="stack" style={{ minWidth: 0, gap: 4 }}>
              <span className="between">
                <span>{(CATS[c.cat] || CATS.other).n}</span>
                <span className="num">
                  {rub(c.spent)} из {rub(c.limit as number)}
                </span>
              </span>
              <Bar value={c.spent} limit={c.limit} />
              <span className={'note' + (c.spent > (c.limit as number) ? ' warn' : '')}>{leftText(c.spent, c.limit as number)}</span>
            </span>
          </div>
        ))}
      {free.length > 0 && (
        <div className="stack" style={{ gap: 2 }}>
          <span className="note">Без лимита</span>
          {free
            .slice()
            .sort((a, b) => b.spent - a.spent)
            .map((c) => (
              <div key={c.cat} className="between" style={{ padding: '4px 0' }}>
                <span className="note" style={{ fontSize: 15, color: 'var(--ink)' }}>
                  {(CATS[c.cat] || CATS.other).n}
                </span>
                <span className="num">{rub(c.spent)}</span>
              </div>
            ))}
        </div>
      )}
      {empty && <p className="empty">Лимиты не заданы. Задайте общий лимит на месяц и, если нужно, — по категориям.</p>}
      <button type="button" className="btn ghost" style={{ alignSelf: 'flex-start' }} onClick={() => setEdit(true)}>
        {empty ? 'Задать лимиты' : 'Изменить лимиты'}
      </button>
    </div>
  );
}

/** Лимиты на месяц: общий и по категориям; пустое поле — без лимита. */
function BudgetEditor({ snap, view, act, onClose }: { snap: Snapshot; view: BudgetView; act: FinanceActions; onClose: () => void }) {
  const [total, setTotal] = useState(view.total ? amountText(view.total) : '');
  const [cats, setCats] = useState<Record<string, string>>(() => Object.fromEntries(view.cats.filter((c) => c.limit != null).map((c) => [c.cat, amountText(c.limit as number)])));
  const [day, setDay] = useState(String(snap.budget_day));
  const [error, setError] = useState<string | null>(null);
  const save = async () => {
    const t = total.trim() ? parseAmount(total) : null;
    if (total.trim() && (t == null || t <= 0)) return setError('Общий лимит — сумма в рублях');
    const out: Record<string, number> = {};
    for (const [c, v] of Object.entries(cats)) {
      if (!v.trim()) continue;
      const k = parseAmount(v);
      if (k == null || k <= 0) return setError(`Лимит «${CATS[c]?.n ?? c}» — сумма в рублях`);
      out[c] = k;
    }
    const b: Budget = { id: view.id, total: t, cats: out };
    if (!(await act.run(api.budgetSave(b), `${NAMES[view.id]} бюджет сохранён`))) return;
    if (day !== String(snap.budget_day)) await act.run(api.settingSet('budget.day', day), day === '1' ? 'Месяц бюджета — с 1-го числа' : `Месяц бюджета — с ${day}-го числа`);
    onClose();
  };
  return (
    <div className="card stack gap12">
      <h2 className="h2">{NAMES[view.id]} бюджет на месяц</h2>
      <label className="label">
        <span>Всего на месяц, ₽</span>
        <input className="field num" inputMode="decimal" value={total} onChange={(e) => setTotal(e.target.value)} placeholder="без лимита" />
      </label>
      <span className="note">По категориям — пустое поле значит «без лимита»</span>
      <div className="form-grid">
        {OWN_OUT.map((c) => (
          <label key={c} className="label">
            <span>{CATS[c].n}</span>
            <input className="field num" inputMode="decimal" value={cats[c] ?? ''} onChange={(e) => setCats({ ...cats, [c]: e.target.value })} placeholder="—" aria-label={`Лимит: ${CATS[c].n}`} />
          </label>
        ))}
      </div>
      <label className="label">
        <span>Месяц бюджета начинается</span>
        <select className="field" value={day} onChange={(e) => setDay(e.target.value)}>
          {Array.from({ length: 28 }, (_, i) => String(i + 1)).map((d) => (
            <option key={d} value={d}>
              {d === '1' ? 'с 1-го числа' : `с ${d}-го числа (например, с дня зарплаты)`}
            </option>
          ))}
        </select>
      </label>
      <ErrorText text={error} />
      <div className="wrap">
        <button type="button" className="btn own" onClick={save}>
          Сохранить
        </button>
        <button type="button" className="btn ghost" onClick={onClose}>
          Отмена
        </button>
      </div>
    </div>
  );
}

export default function BudgetScreen({ snap, act }: { snap: Snapshot; act: FinanceActions }) {
  const [which, setWhich] = useState<'personal' | 'family'>('personal');
  const view = snap.budgets.find((b) => b.id === which);
  return (
    <div className="stack gap16">
      <Seg
        label="Чей бюджет"
        value={which}
        onPick={setWhich}
        items={[
          { id: 'personal', label: 'Личный' },
          { id: 'family', label: 'Семейный' },
        ]}
      />
      <p className="note" style={{ marginTop: -8 }}>
        {which === 'personal'
          ? 'Личные расходы, в том числе с меткой поездки и оплаты по графику. Расходы поездок, переводы и долги не входят.'
          : 'Расходы с меткой «Семья».'}
      </p>
      {view && <BudgetCard key={which} snap={snap} view={view} act={act} />}
    </div>
  );
}
