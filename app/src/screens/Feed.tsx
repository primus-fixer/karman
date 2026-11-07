import { useState } from 'react';
import type { Op, Snapshot } from '../api';
import { addDays, dMon, monthName, rub } from '../format';
import { isOwn, isSpend } from '../logic';
import { OpRow, Seg, type OpActions } from '../ui';

type Scope = 'all' | 'own' | 'trips';

export default function Feed({ snap, act }: { snap: Snapshot; act: OpActions }) {
  const [scope, setScope] = useState<Scope>('all');
  const ops = snap.ops.filter((o) => (scope === 'all' ? true : scope === 'own' ? isOwn(o) : !isOwn(o)));
  const groups: { date: string; ops: Op[]; out: number }[] = [];
  for (const o of ops) {
    let g = groups[groups.length - 1];
    if (!g || g.date !== o.date) {
      g = { date: o.date, ops: [], out: 0 };
      groups.push(g);
    }
    g.ops.push(o);
    if (isSpend(o)) g.out += o.amount;
  }
  const month = snap.today.slice(0, 7);
  const cur = ops.filter((o) => o.date.slice(0, 7) === month);
  const out = cur.filter(isSpend).reduce((s, o) => s + o.amount, 0);
  const income = cur.filter((o) => o.kind === 'income').reduce((s, o) => s + o.amount, 0);
  const company = cur.filter((o) => o.kind === 'advance' || o.kind === 'reimbursement').reduce((s, o) => s + o.amount, 0);
  const summary: string[] = [];
  if (out) summary.push('расходы ' + rub(out));
  if (income && scope !== 'trips') summary.push('доходы ' + rub(income));
  if (company && scope !== 'own') summary.push('от компании ' + rub(company));
  const yesterday = addDays(snap.today, -1);

  return (
    <div className="stack gap12">
      <h1 className="h1">Лента</h1>
      <Seg
        label="Чьи операции показать"
        value={scope}
        onPick={setScope}
        items={[
          { id: 'all', label: 'Всё' },
          { id: 'own', label: 'Личное', tone: 'own' },
          { id: 'trips', label: 'Поездки', tone: 'work' },
        ]}
      />
      <div className="between note">
        <span style={{ flex: 'none' }}>{monthName(snap.today)}</span>
        <span className="num" style={{ textAlign: 'right' }}>
          {summary.join(', ')}
        </span>
      </div>
      {groups.length === 0 && <p className="empty">Операций пока нет. Запишите первую кнопкой «+».</p>}
      <div className="stack">
        {groups.map((g) => (
          <section key={g.date} className="stack">
            <div className="dayhead">
              <h2 style={{ margin: 0, font: 'inherit' }}>{(g.date === snap.today ? 'Сегодня, ' : g.date === yesterday ? 'Вчера, ' : '') + dMon(g.date)}</h2>
              <span className="num">{g.out ? '−' + rub(g.out) : ''}</span>
            </div>
            {g.ops.map((o) => (
              <OpRow key={o.id} snap={snap} op={o} act={act} />
            ))}
          </section>
        ))}
      </div>
    </div>
  );
}
