import { useState, type FormEvent } from 'react';
import type { Snapshot, Trip } from '../api';
import { addDays, amountText, diffDays, parseAmount, rub } from '../format';
import { statusIndex } from '../logic';
import { ErrorText, Icon } from '../ui';

const ARTICLES = [
  ['03.01.04.01', 'Билеты'],
  ['03.01.04.02', 'Представительские'],
  ['03.01.04.03', 'Проживание'],
  ['03.01.04.04', 'Суточные'],
  ['03.01.04.05', 'Транспорт, бензин'],
];

type Draft = {
  id: string;
  city: string;
  reqNo: string;
  reqDate: string;
  region: string;
  goal: string;
  planStart: string;
  planEnd: string;
  start: string;
  end: string;
  deadline: string;
  plan: string[];
  planNotes: string[];
  notes: string[];
};

function toDraft(t: Trip | null, today: string): Draft {
  if (!t) {
    const s = addDays(today, 1);
    const e = addDays(today, 7);
    return { id: '', city: '', reqNo: '', reqDate: today, region: '', goal: '', planStart: s, planEnd: e, start: s, end: e, deadline: '', plan: ['', '', '', '', ''], planNotes: ['', '', '', '', ''], notes: ['', '', '', '', ''] };
  }
  return {
    id: t.id,
    city: t.city,
    reqNo: t.req_no,
    reqDate: t.req_date || '',
    region: t.region,
    goal: t.goal,
    planStart: t.plan_start,
    planEnd: t.plan_end,
    start: t.start,
    end: t.end,
    deadline: t.deadline || '',
    plan: t.plan.map((k) => (k ? amountText(k) : '')),
    planNotes: [...t.plan_notes],
    notes: [...t.notes],
  };
}

export default function TripForm({
  snap,
  tripId,
  onSave,
  onCancel,
}: {
  snap: Snapshot;
  tripId: string | null;
  onSave: (trip: Trip, isNew: boolean) => Promise<boolean>;
  onCancel: () => void;
}) {
  const existing = tripId ? snap.trips.find((x) => x.trip.id === tripId)?.trip ?? null : null;
  const [d, setD] = useState<Draft>(() => toDraft(existing, snap.today));
  const [factTouched, setFactTouched] = useState(!!existing);
  const [showNotes, setShowNotes] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const set = (patch: Partial<Draft>) => setD((x) => ({ ...x, ...patch }));
  const setPlanDate = (key: 'planStart' | 'planEnd', v: string) => setD((x) => ({ ...x, [key]: v, ...(factTouched ? {} : key === 'planStart' ? { start: v } : { end: v }) }));
  const setArr = (key: 'plan' | 'planNotes' | 'notes', i: number, v: string) =>
    setD((x) => {
      const a = [...x[key]];
      a[i] = v;
      return { ...x, [key]: a };
    });

  const planKop = d.plan.map((s) => (s.trim() ? parseAmount(s) : 0));
  const total = planKop.reduce<number>((s, k) => s + (k ?? 0), 0);
  const planDays = d.planStart && d.planEnd && d.planEnd >= d.planStart ? diffDays(d.planStart, d.planEnd) + 1 : 0;
  const rate = existing?.own_rate ?? snap.rate;
  const perDiemHint = planDays > 0 ? planDays * rate : 0;
  const st = existing ? statusIndex(existing.status) : 0;
  const issued = existing ? snap.trips.find((x) => x.trip.id === existing.id)?.s.issued ?? 0 : 0;

  const submit = async (ev: FormEvent) => {
    ev.preventDefault();
    const id = d.id.trim();
    if (!id || !d.city.trim()) return setError('Укажите номер поездки и город');
    if (!existing && snap.trips.some((x) => x.trip.id === id)) return setError(`Поездка ${id} уже есть`);
    if (!d.planStart || !d.planEnd || !d.start || !d.end) return setError('Укажите даты поездки');
    if (d.planEnd < d.planStart || d.end < d.start) return setError('Дата окончания раньше начала');
    const bad = planKop.findIndex((k) => k == null);
    if (bad >= 0) return setError(`Не получается прочитать сумму: ${ARTICLES[bad][1]}`);
    const trip: Trip = {
      id,
      city: d.city.trim(),
      start: d.start,
      end: d.end,
      plan_start: d.planStart,
      plan_end: d.planEnd,
      status: existing ? existing.status : 'request',
      plan: planKop.map((k) => k ?? 0),
      fixed_rate: existing ? existing.fixed_rate : null,
      own_rate: existing ? existing.own_rate : null,
      req_no: d.reqNo.trim(),
      req_date: d.reqDate || null,
      goal: d.goal.trim(),
      region: d.region.trim(),
      plan_notes: d.planNotes.map((s) => s.trim()),
      notes: d.notes.map((s) => s.trim()),
      deadline: d.deadline || null,
      just: existing ? existing.just : null,
    };
    setBusy(true);
    setError(null);
    const ok = await onSave(trip, !existing);
    setBusy(false);
    if (!ok) setError('Не удалось сохранить');
  };

  return (
    <form className="stack gap20" onSubmit={submit} noValidate>
      <div style={{ display: 'flex', alignItems: 'center', gap: 6, margin: '-6px 0 0 -10px' }}>
        <button type="button" className="back" aria-label="Назад" onClick={onCancel}>
          <Icon name="left" size={22} stroke={2} />
        </button>
        <h1 className="h1">{existing ? `Поездка ${existing.id}` : 'Новая поездка'}</h1>
      </div>
      {!existing && <p className="note" style={{ marginTop: -10 }}>Перенесите данные из заявки в 1С. Аванс будет равен сумме заявки, суточные посчитаются по ставке из настроек.</p>}

      <section className="stack gap12">
        <h2 className="h2">Заявка</h2>
        <div className="form-grid">
          <label className="label">
            <span>Номер поездки</span>
            <input className="field num" value={d.id} readOnly={!!existing} onChange={(e) => set({ id: e.target.value })} placeholder="Например, 573" autoFocus={!existing} />
          </label>
          <label className="label">
            <span>Город</span>
            <input className="field" value={d.city} onChange={(e) => set({ city: e.target.value })} placeholder="Например, Нягань" />
          </label>
          <label className="label">
            <span>Номер заявки в 1С</span>
            <input className="field num" value={d.reqNo} onChange={(e) => set({ reqNo: e.target.value })} />
          </label>
          <label className="label">
            <span>Дата заявки</span>
            <input className="field" type="date" value={d.reqDate} onChange={(e) => set({ reqDate: e.target.value })} />
          </label>
          <label className="label">
            <span>Начало по заявке</span>
            <input className="field" type="date" value={d.planStart} onChange={(e) => setPlanDate('planStart', e.target.value)} />
          </label>
          <label className="label">
            <span>Окончание по заявке</span>
            <input className="field" type="date" value={d.planEnd} onChange={(e) => setPlanDate('planEnd', e.target.value)} />
          </label>
        </div>
        <label className="label">
          <span>Описание цели</span>
          <textarea className="field" rows={2} value={d.goal} onChange={(e) => set({ goal: e.target.value })} placeholder="Как в заявке: наладка, монтаж, обучение…" />
        </label>
      </section>

      <section className="stack gap12">
        <h2 className="h2">Фактические даты</h2>
        <div className="form-grid">
          <label className="label">
            <span>Начало</span>
            <input
              className="field"
              type="date"
              value={d.start}
              onChange={(e) => {
                setFactTouched(true);
                set({ start: e.target.value });
              }}
            />
          </label>
          <label className="label">
            <span>Окончание</span>
            <input
              className="field"
              type="date"
              value={d.end}
              onChange={(e) => {
                setFactTouched(true);
                set({ end: e.target.value });
              }}
            />
          </label>
          <label className="label">
            <span>Сдать отчёт до</span>
            <input className="field" type="date" value={d.deadline} onChange={(e) => set({ deadline: e.target.value })} />
          </label>
        </div>
        <p className="note">{factTouched ? 'Суточные считаются по фактическим датам.' : 'Пока совпадают с заявкой. Если поездка сдвинется, поменяйте.'}</p>
      </section>

      <section className="stack gap12">
        <h2 className="h2">Суммы по статьям</h2>
        <div className="stack">
          {ARTICLES.map(([code, name], i) => (
            <div key={code} className="stack gap6" style={{ padding: '10px 0', borderTop: '1px solid var(--line)' }}>
              <div className="plan-grid">
                <label htmlFor={'plan' + i} className="stack" style={{ minWidth: 0 }}>
                  <span>
                    <span className="faint">{code}</span> {name}
                  </span>
                  {i === 3 && perDiemHint > 0 && planKop[3] !== perDiemHint && (
                    <button type="button" className="btn link" style={{ alignSelf: 'flex-start', padding: 0, minHeight: 32, color: 'var(--own)', fontSize: 14 }} onClick={() => setArr('plan', 3, amountText(perDiemHint))}>
                      Подставить {planDays} × {rub(rate)} = {rub(perDiemHint)}
                    </button>
                  )}
                </label>
                <input id={'plan' + i} className="field num" inputMode="decimal" style={{ textAlign: 'right' }} value={d.plan[i]} onChange={(e) => setArr('plan', i, e.target.value)} placeholder="0" />
              </div>
              <input className="field" aria-label={`Примечание к статье ${name}`} value={d.planNotes[i]} onChange={(e) => setArr('planNotes', i, e.target.value)} placeholder="Примечание" style={{ fontSize: 14 }} />
              {i === 2 && <input className="field" aria-label="Регион проживания" value={d.region} onChange={(e) => set({ region: e.target.value })} placeholder="Регион, например ХМАО" style={{ fontSize: 14 }} />}
            </div>
          ))}
          <div className="between" style={{ padding: '11px 0', borderTop: '2px solid var(--ink)' }}>
            <span className="stack">
              <span style={{ fontWeight: 500 }}>Итого по заявке</span>
              <span className="note">{st >= 1 ? `Аванс уже выдан: ${rub(issued)}` : 'Аванс будет равен этой сумме'}</span>
            </span>
            <span className="num" style={{ fontWeight: 500 }}>
              {rub(total)}
            </span>
          </div>
          {st >= 1 && issued !== total && <p className="note warn">Сумма заявки отличается от выданного аванса. Если компания доплатила, запишите доплату как доход в поездку.</p>}
        </div>
      </section>

      <section className="stack gap10">
        <button type="button" className="collapse" aria-expanded={showNotes} onClick={() => setShowNotes(!showNotes)}>
          <span>Примечания к авансовому отчёту</span>
          <Icon name={showNotes ? 'up' : 'down'} size={18} stroke={2} />
        </button>
        {showNotes && (
          <div className="stack gap10">
            <p className="note">Попадут в раздел «Для 1С» → «Авансовый отчёт». Материалы и инструменты дописываются к статье 05 сами.</p>
            {ARTICLES.map(([code, name], i) => (
              <label key={code} className="label">
                <span>
                  {code} {name}
                </span>
                <input className="field" value={d.notes[i]} onChange={(e) => setArr('notes', i, e.target.value)} />
              </label>
            ))}
          </div>
        )}
      </section>

      <ErrorText text={error} />
      <div className="wrap">
        <button type="submit" className="btn work" disabled={busy}>
          {existing ? 'Сохранить' : 'Создать поездку'}
        </button>
        <button type="button" className="btn ghost" onClick={onCancel}>
          Отмена
        </button>
      </div>
    </form>
  );
}
