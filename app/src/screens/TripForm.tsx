import { useState, type FormEvent } from 'react';
import { api, message, type Attachment, type Snapshot, type Trip } from '../api';
import { aiOn, useAi } from '../aiState';
import { addDays, amountText, diffDays, parseAmount, rub } from '../format';
import { statusIndex } from '../logic';
import { attachFile, pickFiles } from '../media';
import { ErrorText, Icon } from '../ui';

/** Снимок приказа или заявки — как наряд: мелкий текст должен читаться. */
const DOC_PHOTO = { side: 2200, name: 'приказ.jpg' };

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
  mobile = false,
  onSave,
  onCancel,
}: {
  snap: Snapshot;
  tripId: string | null;
  /** Телефон: приказ можно снять камерой. */
  mobile?: boolean;
  onSave: (trip: Trip, isNew: boolean) => Promise<boolean>;
  onCancel: () => void;
}) {
  const ai = useAi();
  const [scan, setScan] = useState<{ busy: boolean; note: string; warn: boolean } | null>(null);
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

  /** Приказ или заявка → поля формы (Claude Opus 5.5). Сохраняет человек. */
  const fromDocument = async (camera: boolean) => {
    const files = await pickFiles({ accept: camera ? 'image/*' : 'image/*,application/pdf', capture: camera, multiple: !camera });
    if (!files.length) return;
    setScan({ busy: true, note: 'Claude читает документ — обычно до минуты…', warn: false });
    try {
      const att: Attachment[] = [];
      for (const f of files) att.push(await attachFile(f, DOC_PHOTO));
      const r = await api.aiTripScan(att.map((a) => a.id));
      setD((x) => ({
        ...x,
        id: r.id && !snap.trips.some((t) => t.trip.id === r.id) ? r.id : x.id,
        city: r.city || x.city,
        region: r.region || x.region,
        reqNo: r.req_no || x.reqNo,
        reqDate: r.req_date || x.reqDate,
        goal: r.goal || x.goal,
        planStart: r.start || x.planStart,
        planEnd: r.end || x.planEnd,
        start: factTouched ? x.start : r.start || x.start,
        end: factTouched ? x.end : r.end || x.end,
        plan: x.plan.map((v, i) => (r.plan[i] > 0 ? amountText(r.plan[i]) : v)),
      }));
      const parts = ['Заполнено по документу — проверьте и сохраните'];
      if (!r.id) parts.push('номер поездки впишите сами');
      if (r.employee) parts.push('командируемый: ' + r.employee);
      setScan({ busy: false, note: parts.join('; ') + (r.problems ? '. Claude не уверен: ' + r.problems : ''), warn: !!r.problems });
    } catch (e) {
      setScan({ busy: false, note: message(e), warn: true });
    }
  };

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
        <h1 className="screen-title">{existing ? `Поездка ${existing.id}` : 'Новая поездка'}</h1>
      </div>
      {!existing && <p className="small muted" style={{ marginTop: -10 }}>Перенесите данные из заявки в 1С. Аванс будет равен сумме заявки, суточные посчитаются по ставке из настроек.</p>}
      {!existing && aiOn(ai, 'docs') && (
        <section className="card stack gap6" aria-label="Из приказа или заявки">
          <span style={{ display: 'flex', alignItems: 'center', gap: 6, fontWeight: 500 }}>
            <Icon name="spark" size={18} />
            Из приказа или заявки
          </span>
          <span className="small muted">Снимок или PDF — Claude заполнит номер, город, даты, цель и суммы по статьям. Проверьте перед сохранением.</span>
          <div className="sec-actions">
            {mobile && (
              <button type="button" className="btn line" style={{ display: 'flex', alignItems: 'center', gap: 6 }} disabled={scan?.busy} onClick={() => fromDocument(true)}>
                <Icon name="camera" size={18} />
                Снять
              </button>
            )}
            <button type="button" className="btn line" style={{ display: 'flex', alignItems: 'center', gap: 6 }} disabled={scan?.busy} onClick={() => fromDocument(false)}>
              <Icon name="file" size={18} />
              {mobile ? 'Из файлов' : 'Снимок или PDF'}
            </button>
          </div>
          {scan && (
            <p className={'small muted' + (scan.warn ? ' warn-text' : '')} role="status">
              {scan.note}
            </p>
          )}
        </section>
      )}

      <section className="stack gap12">
        <h2 className="sec">Заявка</h2>
        <div className="form-grid">
          <label className="field">
            <span>Номер поездки</span>
            <input className="input num" value={d.id} readOnly={!!existing} onChange={(e) => set({ id: e.target.value })} placeholder="Например, 573" autoFocus={!existing} />
          </label>
          <label className="field">
            <span>Город</span>
            <input className="input" value={d.city} onChange={(e) => set({ city: e.target.value })} placeholder="Например, Нягань" />
          </label>
          <label className="field">
            <span>Номер заявки в 1С</span>
            <input className="input num" value={d.reqNo} onChange={(e) => set({ reqNo: e.target.value })} />
          </label>
          <label className="field">
            <span>Дата заявки</span>
            <input className="input" type="date" value={d.reqDate} onChange={(e) => set({ reqDate: e.target.value })} />
          </label>
          <label className="field">
            <span>Начало по заявке</span>
            <input className="input" type="date" value={d.planStart} onChange={(e) => setPlanDate('planStart', e.target.value)} />
          </label>
          <label className="field">
            <span>Окончание по заявке</span>
            <input className="input" type="date" value={d.planEnd} onChange={(e) => setPlanDate('planEnd', e.target.value)} />
          </label>
        </div>
        <label className="field">
          <span>Описание цели</span>
          <textarea className="input" rows={2} value={d.goal} onChange={(e) => set({ goal: e.target.value })} placeholder="Как в заявке: наладка, монтаж, обучение…" />
        </label>
      </section>

      <section className="stack gap12">
        <h2 className="sec">Фактические даты</h2>
        <div className="form-grid">
          <label className="field">
            <span>Начало</span>
            <input
              className="input"
              type="date"
              value={d.start}
              onChange={(e) => {
                setFactTouched(true);
                set({ start: e.target.value });
              }}
            />
          </label>
          <label className="field">
            <span>Окончание</span>
            <input
              className="input"
              type="date"
              value={d.end}
              onChange={(e) => {
                setFactTouched(true);
                set({ end: e.target.value });
              }}
            />
          </label>
          <label className="field">
            <span>Сдать отчёт до</span>
            <input className="input" type="date" value={d.deadline} onChange={(e) => set({ deadline: e.target.value })} />
          </label>
        </div>
        <p className="small muted">{factTouched ? 'Суточные считаются по фактическим датам.' : 'Пока совпадают с заявкой. Если поездка сдвинется, поменяйте.'}</p>
      </section>

      <section className="stack gap12">
        <h2 className="sec">Суммы по статьям</h2>
        <div className="stack gap0">
          {ARTICLES.map(([code, name], i) => (
            <div key={code} className="stack gap6" style={{ padding: '10px 0', borderTop: '1px solid var(--line)' }}>
              <div className="plan-grid">
                <label htmlFor={'plan' + i} className="stack gap0" style={{ minWidth: 0 }}>
                  <span>
                    <span className="muted3">{code}</span> {name}
                  </span>
                  {i === 3 && perDiemHint > 0 && planKop[3] !== perDiemHint && (
                    <button type="button" className="link-btn" style={{ alignSelf: 'flex-start', minHeight: 32, fontSize: 14 }} onClick={() => setArr('plan', 3, amountText(perDiemHint))}>
                      Подставить {planDays} × {rub(rate)} = {rub(perDiemHint)}
                    </button>
                  )}
                </label>
                <input id={'plan' + i} className="input num" inputMode="decimal" style={{ textAlign: 'right' }} value={d.plan[i]} onChange={(e) => setArr('plan', i, e.target.value)} placeholder="0" />
              </div>
              <input className="input" aria-label={`Примечание к статье ${name}`} value={d.planNotes[i]} onChange={(e) => setArr('planNotes', i, e.target.value)} placeholder="Примечание" style={{ fontSize: 14 }} />
              {i === 2 && <input className="input" aria-label="Регион проживания" value={d.region} onChange={(e) => set({ region: e.target.value })} placeholder="Регион, например ХМАО" style={{ fontSize: 14 }} />}
            </div>
          ))}
          <div className="between" style={{ padding: '11px 0', borderTop: '2px solid var(--ink)' }}>
            <span className="stack gap0">
              <span style={{ fontWeight: 500 }}>Итого по заявке</span>
              <span className="small muted">{st >= 1 ? `Аванс уже выдан: ${rub(issued)}` : 'Аванс будет равен этой сумме'}</span>
            </span>
            <span className="num" style={{ fontWeight: 500 }}>
              {rub(total)}
            </span>
          </div>
          {st >= 1 && issued !== total && <p className="small warn-text">Сумма заявки отличается от выданного аванса. Если компания доплатила, запишите доплату как доход в поездку.</p>}
        </div>
      </section>

      <section className="stack gap10">
        <button type="button" className="collapse" aria-expanded={showNotes} onClick={() => setShowNotes(!showNotes)}>
          <span>Примечания к авансовому отчёту</span>
          <Icon name={showNotes ? 'up' : 'down'} size={18} stroke={2} />
        </button>
        {showNotes && (
          <div className="stack gap10">
            <p className="small muted">Попадут в раздел «Для 1С» → «Авансовый отчёт». Материалы и инструменты дописываются к статье 05 сами.</p>
            {ARTICLES.map(([code, name], i) => (
              <label key={code} className="field">
                <span>
                  {code} {name}
                </span>
                <input className="input" value={d.notes[i]} onChange={(e) => setArr('notes', i, e.target.value)} />
              </label>
            ))}
          </div>
        )}
      </section>

      <ErrorText text={error} />
      <div className="sec-actions">
        <button type="submit" className="btn work" disabled={busy}>
          {existing ? 'Сохранить' : 'Создать поездку'}
        </button>
        <button type="button" className="btn line" onClick={onCancel}>
          Отмена
        </button>
      </div>
    </form>
  );
}
