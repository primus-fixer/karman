import { useEffect, useState } from 'react';
import type { Op, Snapshot, Trip, TripView } from '../api';
import type { Nav } from '../App';
import { daysText, dMon, dNum, dRange, fmt2, parseAmount, plain, plural, rub } from '../format';
import { DOC_LABEL, NEXT, statusIndex, tripControls, tripSummary } from '../logic';
import { Icon, OpRow, Rail, Seg, Thumbs, type OpActions } from '../ui';

export type TripTab = 'summary' | 'report' | 'ops' | 'docs';

export type TripActions = {
  advance: (id: string) => void;
  settle: (id: string) => void;
  save: (trip: Trip) => void;
  remove: (id: string) => void;
  addDoc: (op: Op) => void;
  toggleSent: (op: Op) => void;
  addOp: (tripId: string) => void;
  copy: (text: string, label: string) => void;
  /** Архив снимков и файлов чеков поездки (на компьютере). */
  zip: (tripId: string) => void;
};

function CopyField({ label, value, copy }: { label: string; value: string; copy: TripActions['copy'] }) {
  return (
    <button type="button" className="field-row copy" onClick={() => copy(value, label)}>
      <span>{label}</span>
      <span style={{ lineHeight: '20px' }}>{value || '—'}</span>
    </button>
  );
}

function Summary({ snap, tv, nav, act, goTab }: { snap: Snapshot; tv: TripView; nav: Nav; act: TripActions; goTab: (tab: TripTab, part?: 'plan' | 'fact') => void }) {
  const t = tv.trip;
  const st = statusIndex(t.status);
  const v = tripSummary(snap, tv);
  const controls = tripControls(snap, tv, snap.ops);
  const [rateText, setRateText] = useState(t.own_rate != null ? String(t.own_rate / 100) : '');
  useEffect(() => setRateText(t.own_rate != null ? String(t.own_rate / 100) : ''), [t.own_rate]);
  const commitRate = () => {
    const k = rateText.trim() === '' ? null : parseAmount(rateText);
    if (rateText.trim() !== '' && k == null) return;
    if (k !== t.own_rate) act.save({ ...t, own_rate: k });
  };
  return (
    <>
      <div className="cards">
        <section className="card">
          <h2 className="h2" style={{ marginBottom: 8 }}>
            Расчёт с компанией
          </h2>
          {v.company.map((l) => (
            <div className="line" key={l.label}>
              <span>{l.label}</span>
              <span>{l.value}</span>
            </div>
          ))}
          <div className="result">
            <span>{v.resultLabel}</span>
            <span className={v.resultWarn ? 'warn' : ''}>{v.resultValue}</span>
          </div>
          {v.split.map((s) => (
            <div className={'between' + (s.warn ? ' warn' : '')} key={s.label} style={{ fontSize: 14, lineHeight: '20px', marginTop: 2 }}>
              <span className={s.warn ? '' : 'note'} style={{ fontSize: 14 }}>
                {s.label}
              </span>
              <span className="num" style={{ flex: 'none' }}>
                {s.value}
              </span>
            </div>
          ))}
          {v.companyNote && (
            <p className={'note' + (v.companyNoteWarn ? ' warn' : '')} style={{ display: 'flex', gap: 8, marginTop: 8 }}>
              <span style={{ flex: 'none', marginTop: 1 }}>
                <Icon name="alert" size={15} stroke={2} />
              </span>
              <span>{v.companyNote}</span>
            </p>
          )}
          {v.canSettle && (
            <button type="button" className="btn work" style={{ marginTop: 10 }} onClick={() => act.settle(t.id)}>
              {v.settleLabel}
            </button>
          )}
        </section>
        <section className="card">
          <h2 className="h2" style={{ marginBottom: 6 }}>
            Суточные
          </h2>
          {st < 5 ? (
            <label className="between" style={{ alignItems: 'center', minHeight: 48, marginBottom: 6, paddingBottom: 8, borderBottom: '1px solid var(--line)' }}>
              <span className="stack" style={{ minWidth: 0 }}>
                <span>Ставка в день, ₽</span>
                <span className="note">{v.rateNote}</span>
              </span>
              <input
                className="field num"
                inputMode="decimal"
                aria-label="Своя ставка суточных в этой поездке, рублей в день"
                placeholder={String(tv.rate / 100)}
                value={rateText}
                onChange={(e) => setRateText(e.target.value)}
                onBlur={commitRate}
                onKeyDown={(e) => e.key === 'Enter' && (e.target as HTMLInputElement).blur()}
                style={{ width: 88, height: 40, textAlign: 'right', fontSize: 16 }}
              />
            </label>
          ) : (
            <p className="note" style={{ marginBottom: 6 }}>
              Ставка {rub(tv.rate)} в день, зафиксирована при закрытии
            </p>
          )}
          {v.self.map((l) => (
            <div className="line" key={l.label}>
              <span>{l.label}</span>
              <span>{l.value}</span>
            </div>
          ))}
          <div className="result">
            <span>{v.leftLabel}</span>
            <span className={v.leftWarn ? 'warn' : ''}>{v.leftValue}</span>
          </div>
          {v.leftNote && (
            <p className="note" style={{ marginTop: 6 }}>
              {v.leftNote}
            </p>
          )}
          {v.perDay && (
            <p className="note" style={{ marginTop: 6 }}>
              {v.perDay}
            </p>
          )}
          {v.tagged && (
            <p className="note" style={{ marginTop: 6 }}>
              Личные траты с меткой поездки: {v.tagged}. В суточные не входят.
            </p>
          )}
        </section>
      </div>
      <section className="stack">
        <h2 className="h2" style={{ marginBottom: 6 }}>
          Что проконтролировать
        </h2>
        {controls.map((c) => (
          <button
            key={c.label}
            type="button"
            className="listbtn"
            onClick={() => (c.go === 'docs' ? goTab('docs') : c.go === 'report-plan' ? goTab('report', 'plan') : c.go === 'report-fact' ? goTab('report', 'fact') : goTab('summary'))}
          >
            <span>{c.label}</span>
            <span className={c.warn ? 'warn' : 'note'} style={{ display: 'flex', alignItems: 'center', gap: 6, flex: 'none', fontSize: 15 }}>
              {c.warn && <Icon name="alert" size={16} stroke={2} />}
              {c.value}
            </span>
          </button>
        ))}
      </section>
      <div className="wrap">
        <button type="button" className="btn ghost" onClick={() => nav.editTrip(t.id)}>
          Изменить поездку
        </button>
      </div>
    </>
  );
}

function Report({ snap, tv, act, part, setPart, isPhone }: { snap: Snapshot; tv: TripView; act: TripActions; part: 'plan' | 'fact'; setPart: (p: 'plan' | 'fact') => void; isPhone: boolean }) {
  const t = tv.trip;
  const c = tv.s;
  const copy = act.copy;
  const just = t.just ?? tv.auto_just;
  const [justText, setJustText] = useState(just);
  useEffect(() => setJustText(t.just ?? tv.auto_just), [t.just, tv.auto_just]);
  const excluded = snap.ops.filter((o) => o.trip === t.id && o.kind === 'work' && !o.doc).sort((a, b) => (a.date < b.date ? -1 : 1));
  const totalPlan = tv.rows.reduce((s, r) => s + r.plan, 0);
  const totalFact = tv.rows.reduce((s, r) => s + r.fact, 0);
  const spent = c.confirmed + c.per_diem;
  const rest = c.issued - spent;
  const toolsSum = snap.ops.filter((o) => o.trip === t.id && o.kind === 'work' && o.doc && o.cat === 'tools').reduce((s, o) => s + o.amount, 0);
  const factNote = (i: number) => {
    let n = t.notes[i] || '';
    if (i === 4 && toolsSum > 0) n += (n ? '; ' : '') + 'материалы и инструменты ' + rub(toolsSum);
    return n;
  };
  const advanceNote = c.issued === totalPlan && totalPlan > 0 ? 'Аванс выдан на всю сумму заявки' : c.issued ? 'Выдано авансом ' + rub(c.issued) : 'Аванс ещё не выдан';
  const overRows = tv.rows.filter((r) => r.over > 0);

  return (
    <div className="stack gap16">
      <Seg
        label="Документ для 1С"
        value={part}
        onPick={setPart}
        items={[
          { id: 'plan', label: 'Заявка' },
          { id: 'fact', label: 'Авансовый отчёт' },
        ]}
      />
      {part === 'plan' ? (
        <>
          <section className="card stack gap10" style={{ padding: '12px 14px', borderRadius: 14, background: 'none' }}>
            <div className="between" style={{ alignItems: 'center' }}>
              <h3 style={{ margin: 0, fontSize: 15, fontWeight: 500 }}>Сверх бюджета</h3>
              {tv.over_total > 0 && (
                <button type="button" className="btn link num copy" style={{ color: 'var(--warn)', fontSize: 16, fontWeight: 500, padding: 0 }} onClick={() => copy(plain(tv.over_total), 'сверх бюджета ' + fmt2(tv.over_total))}>
                  +{fmt2(tv.over_total)}
                </button>
              )}
            </div>
            {tv.over_total > 0 ? (
              <>
                {overRows.map((r) => (
                  <div className="between" key={r.code} style={{ fontSize: 14 }}>
                    <span className="note" style={{ fontSize: 14 }}>
                      {r.name}
                    </span>
                    <span className="num">+{fmt2(r.over)}</span>
                  </div>
                ))}
                <label className="label">
                  <span>Обоснование</span>
                  <textarea
                    className="field"
                    rows={3}
                    value={justText}
                    onChange={(e) => setJustText(e.target.value)}
                    onBlur={() => {
                      if (justText !== just) act.save({ ...t, just: justText === tv.auto_just ? null : justText });
                    }}
                  />
                </label>
                <button type="button" className="btn ghost" style={{ alignSelf: 'flex-start' }} onClick={() => copy(justText, 'обоснование')}>
                  Скопировать обоснование
                </button>
              </>
            ) : (
              <p className="note">{c.finished ? 'Факт не выходит за заявку, обоснование не нужно.' : 'Пока в пределах заявки.'}</p>
            )}
          </section>
          <div className="dl">
            <CopyField label="Наименование" value={`${t.id} ${t.city}`} copy={copy} />
            <CopyField label="Номер заявки" value={t.req_no} copy={copy} />
            <CopyField label="Дата заявки" value={t.req_date ? dNum(t.req_date) : ''} copy={copy} />
            <CopyField label="Цель заявки" value="Командировочные расходы" copy={copy} />
            <CopyField label="Начало план" value={dNum(t.plan_start)} copy={copy} />
            <CopyField label="Окончание план" value={dNum(t.plan_end)} copy={copy} />
            <CopyField label="Описание цели" value={t.goal} copy={copy} />
          </div>
          <p className="note">Нажмите на поле, сумму или примечание, чтобы скопировать для 1С.</p>
          {isPhone ? (
            <div className="stack">
              {tv.rows.map((r, i) => (
                <div key={r.code} className="stack" style={{ padding: '8px 0', borderTop: '1px solid var(--line)' }}>
                  <div className="between" style={{ alignItems: 'center' }}>
                    <span style={{ display: 'flex', gap: 8, alignItems: 'baseline', minWidth: 0 }}>
                      <span className="note faint" style={{ flex: 'none' }}>
                        {r.code}
                      </span>
                      <span style={{ fontWeight: 500 }}>{r.name}</span>
                    </span>
                    <button type="button" className="btn link num copy" style={{ color: 'var(--ink)', fontWeight: 500, padding: 0 }} onClick={() => copy(plain(r.plan), 'план ' + fmt2(r.plan))}>
                      {r.plan ? fmt2(r.plan) : '—'}
                    </button>
                  </div>
                  <button type="button" className="btn link copy note" style={{ color: 'var(--ink2)', fontSize: 13, padding: 0, minHeight: 32, textAlign: 'left' }} onClick={() => copy(t.plan_notes[i] || '', 'примечание')}>
                    {t.plan_notes[i] || '—'}
                  </button>
                </div>
              ))}
              <div className="between" style={{ padding: '11px 0', borderTop: '2px solid var(--ink)' }}>
                <span className="stack">
                  <span style={{ fontWeight: 500 }}>Итого по заявке</span>
                  <span className="note">{advanceNote}</span>
                </span>
                <span className="num" style={{ fontWeight: 500 }}>
                  {fmt2(totalPlan)}
                </span>
              </div>
            </div>
          ) : (
            <div style={{ overflowX: 'auto' }}>
              <div className="table" role="table" aria-label="Заявка по статьям">
                <div className="tr head" role="row" style={{ gridTemplateColumns: '30px minmax(190px,1.3fr) 110px 72px minmax(220px,2fr)' }}>
                  <span>N</span>
                  <span>Статья</span>
                  <span className="right">План</span>
                  <span>Регион</span>
                  <span>Примечание</span>
                </div>
                {tv.rows.map((r, i) => (
                  <div className="tr" role="row" key={r.code} style={{ gridTemplateColumns: '30px minmax(190px,1.3fr) 110px 72px minmax(220px,2fr)' }}>
                    <span className="faint">{i + 1}</span>
                    <span>
                      <span className="faint">{r.code}</span> {r.name}
                    </span>
                    <span className="right">
                      <button type="button" className="btn link num copy" style={{ color: 'var(--ink)', fontWeight: 500, padding: 0 }} onClick={() => copy(plain(r.plan), 'план ' + fmt2(r.plan))}>
                        {r.plan ? fmt2(r.plan) : '—'}
                      </button>
                    </span>
                    <span>{i === 2 ? t.region : ''}</span>
                    <span>
                      <button type="button" className="btn link copy" style={{ color: 'var(--ink)', padding: 0, textAlign: 'left' }} onClick={() => copy(t.plan_notes[i] || '', 'примечание')}>
                        {t.plan_notes[i] || '—'}
                      </button>
                    </span>
                  </div>
                ))}
                <div className="tr total" role="row" style={{ gridTemplateColumns: '30px minmax(190px,1.3fr) 110px 72px minmax(220px,2fr)' }}>
                  <span />
                  <span>Итого по заявке</span>
                  <span className="right num">{fmt2(totalPlan)}</span>
                  <span />
                  <span className="note" style={{ fontWeight: 400 }}>
                    {advanceNote}
                  </span>
                </div>
              </div>
            </div>
          )}
        </>
      ) : (
        <>
          {excluded.length > 0 && (
            <section className="warnbox">
              <h2 className="h2 warn" style={{ fontSize: 16 }}>
                Не вошло в отчёт
              </h2>
              <p className="note" style={{ color: 'var(--ink)', marginBottom: 4 }}>
                {excluded.length} {plural(excluded.length, 'расход', 'расхода', 'расходов')} без документов на {rub(c.no_doc)} не входят в отчёт: сначала вычитаются из суточных, остальное — долг перед компанией.
                Добавьте документ, и расход войдёт в отчёт.
              </p>
              {excluded.map((o) => (
                <div key={o.id} className="between" style={{ alignItems: 'center', minHeight: 52, padding: '6px 0', borderTop: '1px solid var(--line)' }}>
                  <span className="stack" style={{ minWidth: 0 }}>
                    <span>{o.title}</span>
                    <span className="note">
                      {dMon(o.date)}, {rub(o.amount)}
                    </span>
                  </span>
                  <button type="button" className="btn ghost" style={{ flex: 'none', minHeight: 40, borderColor: 'var(--warn)', color: 'var(--warn)' }} onClick={() => act.addDoc(o)}>
                    Документ есть
                  </button>
                </div>
              ))}
            </section>
          )}
          <div className="dl">
            <CopyField label="Номер заявки" value={t.req_no} copy={copy} />
            <CopyField label="Начало факт" value={dNum(t.start)} copy={copy} />
            <CopyField label="Окончание факт" value={dNum(t.end)} copy={copy} />
          </div>
          {isPhone ? (
            <div className="stack">
              {tv.rows.map((r, i) => (
                <div key={r.code} className="stack gap6" style={{ padding: '11px 0', borderTop: '1px solid var(--line)' }}>
                  <div style={{ display: 'flex', gap: 8, alignItems: 'baseline' }}>
                    <span className="note faint" style={{ flex: 'none' }}>
                      {r.code}
                    </span>
                    <span style={{ fontWeight: 500 }}>{r.name}</span>
                  </div>
                  <div style={{ display: 'grid', gridTemplateColumns: 'repeat(2, minmax(0,1fr))', gap: 12 }}>
                    <span className="stack">
                      <span className="note">План</span>
                      <span className="num">{r.plan ? fmt2(r.plan) : '—'}</span>
                    </span>
                    <button type="button" className="copy" style={{ display: 'flex', flexDirection: 'column', alignItems: 'flex-start', minHeight: 44, padding: 0, border: 0, background: 'none', textAlign: 'left' }} onClick={() => copy(plain(r.fact), fmt2(r.fact))}>
                      <span className="note">Факт</span>
                      <span className="num" style={{ fontWeight: 500 }}>
                        {r.fact ? fmt2(r.fact) : '—'}
                      </span>
                      {r.over > 0 && <span className="note warn">сверх плана +{fmt2(r.over)}</span>}
                    </button>
                  </div>
                  <button type="button" className="copy note" style={{ minHeight: 40, padding: '4px 0', border: 0, background: 'none', textAlign: 'left' }} onClick={() => copy(factNote(i), 'примечание')}>
                    {factNote(i) || '—'}
                    {i === 2 && t.region ? ` Регион: ${t.region}.` : ''}
                  </button>
                </div>
              ))}
              <div style={{ display: 'grid', gridTemplateColumns: 'repeat(2, minmax(0,1fr))', gap: 12, padding: '11px 0', borderTop: '2px solid var(--ink)' }}>
                <span className="stack">
                  <span className="note">Итого план</span>
                  <span className="num" style={{ fontWeight: 500 }}>
                    {fmt2(totalPlan)}
                  </span>
                </span>
                <span className="stack">
                  <span className="note">Итого факт</span>
                  <span className="num" style={{ fontWeight: 500 }}>
                    {fmt2(totalFact)}
                  </span>
                </span>
              </div>
            </div>
          ) : (
            <div style={{ overflowX: 'auto' }}>
              <div className="table" role="table" aria-label="Расходы по статьям">
                <div className="tr head" role="row">
                  <span>N</span>
                  <span>Статья</span>
                  <span className="right">План</span>
                  <span className="right">Факт</span>
                  <span>Регион</span>
                  <span>Примечание</span>
                </div>
                {tv.rows.map((r, i) => (
                  <div className="tr" role="row" key={r.code}>
                    <span className="faint">{i + 1}</span>
                    <span>
                      <span className="faint">{r.code}</span> {r.name}
                    </span>
                    <span className="right num">{r.plan ? fmt2(r.plan) : '—'}</span>
                    <span className="right" style={{ display: 'flex', flexDirection: 'column', alignItems: 'flex-end' }}>
                      <button type="button" className="btn link num copy" style={{ color: 'var(--ink)', fontWeight: 500, padding: 0, minHeight: 36 }} onClick={() => copy(plain(r.fact), fmt2(r.fact))}>
                        {r.fact ? fmt2(r.fact) : '—'}
                      </button>
                      {r.over > 0 && <span className="warn" style={{ fontSize: 12, lineHeight: '16px' }}>сверх +{fmt2(r.over)}</span>}
                    </span>
                    <span>{i === 2 ? t.region : ''}</span>
                    <span>
                      <button type="button" className="btn link copy" style={{ color: 'var(--ink)', padding: 0, textAlign: 'left' }} onClick={() => copy(factNote(i), 'примечание')}>
                        {factNote(i) || '—'}
                      </button>
                    </span>
                  </div>
                ))}
                <div className="tr total" role="row">
                  <span />
                  <span>Итого</span>
                  <span className="right num">{fmt2(totalPlan)}</span>
                  <span className="right num">{fmt2(totalFact)}</span>
                </div>
              </div>
            </div>
          )}
          <div className="settle">
            <button type="button" onClick={() => copy(plain(c.issued), 'получено ' + fmt2(c.issued))}>
              <span className="note">Получено</span>
              <span className="num" style={{ fontWeight: 500 }}>
                {fmt2(c.issued)}
              </span>
            </button>
            <button type="button" onClick={() => copy(plain(spent), 'израсходовано ' + fmt2(spent))}>
              <span className="note">Израсходовано</span>
              <span className="num" style={{ fontWeight: 500 }}>
                {fmt2(spent)}
              </span>
            </button>
            <button type="button" onClick={() => copy(plain(Math.abs(rest)), (rest >= 0 ? 'остаток ' : 'перерасход ') + fmt2(Math.abs(rest)))}>
              <span className="note">{rest >= 0 ? 'Остаток' : 'Перерасход'}</span>
              <span className={'num' + (rest > 0 ? ' warn' : '')} style={{ fontWeight: 500 }}>
                {fmt2(Math.abs(rest))}
              </span>
            </button>
          </div>
          {tv.over_total > 0 && (
            <button type="button" className="listbtn" style={{ fontSize: 14 }} onClick={() => setPart('plan')}>
              <span>Сверх заявки {rub(tv.over_total)}: обоснование — в разделе «Заявка».</span>
              <Icon name="right" size={18} stroke={2} />
            </button>
          )}
        </>
      )}
    </div>
  );
}

export default function TripScreen({
  snap,
  tripId,
  nav,
  act,
  tab,
  setTab,
  part,
  setPart,
  isPhone,
  onBack,
  opAct,
}: {
  snap: Snapshot;
  tripId: string;
  nav: Nav;
  act: TripActions;
  tab: TripTab;
  setTab: (t: TripTab) => void;
  part: 'plan' | 'fact' | null;
  setPart: (p: 'plan' | 'fact') => void;
  isPhone: boolean;
  onBack: () => void;
  opAct: OpActions;
}) {
  const [confirmDelete, setConfirmDelete] = useState(false);
  const tv = snap.trips.find((x) => x.trip.id === tripId);
  if (!tv) {
    return (
      <div className="stack gap16">
        <p className="empty">Поездка удалена.</p>
        <button type="button" className="btn ghost" onClick={onBack}>
          К поездкам
        </button>
      </div>
    );
  }
  const t = tv.trip;
  const st = statusIndex(t.status);
  const ops = snap.ops.filter((o) => o.trip === t.id);
  const work = ops.filter((o) => o.kind === 'work').sort((a, b) => (a.date < b.date ? -1 : 1));
  const withDoc = work.filter((o) => o.doc).length;
  const sent = work.filter((o) => o.doc && o.sent).length;
  const curPart = part || (st >= 3 ? 'fact' : 'plan');
  const goTab = (x: TripTab, p?: 'plan' | 'fact') => {
    setTab(x);
    if (p) setPart(p);
  };
  return (
    <div className="stack gap16">
      <div style={{ display: 'flex', alignItems: 'flex-start', gap: 6, margin: '-6px 0 0 -10px' }}>
        <button type="button" className="back" aria-label="Назад" onClick={onBack}>
          <Icon name="left" size={22} stroke={2} />
        </button>
        <div style={{ paddingTop: 4, minWidth: 0 }}>
          <h1 className="h1">
            {t.id} {t.city}
          </h1>
          <p className="note">
            {dRange(t.start, t.end)}, {daysText(tv.s.days)}
          </p>
        </div>
      </div>
      <Rail statusIdx={st} />
      {st < 5 && (
        <button type="button" className="btn ghost" onClick={() => act.advance(t.id)}>
          {NEXT[st]}
        </button>
      )}
      <div className="tabs" role="tablist" aria-label="Разделы поездки">
        {(
          [
            ['summary', 'Итоги'],
            ['report', 'Для 1С'],
            ['ops', 'Операции'],
            ['docs', 'Документы'],
          ] as [TripTab, string][]
        ).map(([id, label]) => (
          <button key={id} type="button" role="tab" aria-selected={tab === id} className={tab === id ? 'on' : ''} onClick={() => setTab(id)}>
            {label}
          </button>
        ))}
      </div>

      {tab === 'summary' && <Summary snap={snap} tv={tv} nav={nav} act={act} goTab={goTab} />}
      {tab === 'report' && <Report snap={snap} tv={tv} act={act} part={curPart} setPart={setPart} isPhone={isPhone} />}
      {tab === 'ops' && (
        <div className="stack">
          {st < 5 && (
            <button type="button" className="btn wide" style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', gap: 8, marginBottom: 12 }} onClick={() => act.addOp(t.id)}>
              <Icon name="plus" size={20} stroke={2} />
              Записать расход в эту поездку
            </button>
          )}
          {ops.length === 0 && <p className="empty">Операций по поездке пока нет.</p>}
          {ops.map((o) => (
            <OpRow key={o.id} snap={snap} op={o} act={opAct} dateSub />
          ))}
        </div>
      )}
      {tab === 'docs' && (
        <div className="stack">
          <p style={{ margin: 0 }}>{work.length ? `С документами ${withDoc} из ${work.length}` + (withDoc ? `, отправлено в офис ${sent} из ${withDoc}` : '') : 'Документы'}</p>
          <p className="note" style={{ margin: '2px 0 10px' }}>
            Отметьте документы, которые отправили в офис. Расход без документа вычитается из суточных, а сверх них становится долгом перед компанией.
          </p>
          {!isPhone && work.some((o) => o.att?.length) && (
            <button type="button" className="btn ghost" style={{ alignSelf: 'flex-start', display: 'flex', alignItems: 'center', gap: 8, marginBottom: 10 }} onClick={() => act.zip(t.id)}>
              <Icon name="download" size={18} />
              Архив чеков в «Загрузки»
            </button>
          )}
          {work.length === 0 && <p className="empty">Рабочих расходов пока нет.</p>}
          {work.map((o) =>
            o.doc ? (
              <div key={o.id} style={{ borderTop: '1px solid var(--line)', padding: '6px 0' }}>
                <div style={{ display: 'grid', gridTemplateColumns: '24px minmax(0,1fr) auto', gap: 12, alignItems: 'center', minHeight: 44 }}>
                  <input type="checkbox" aria-label={`Отправлено в офис: ${o.title}`} checked={o.sent} onChange={() => act.toggleSent(o)} style={{ width: 20, height: 20, margin: 0, accentColor: 'var(--work)' }} />
                  <span className="stack" style={{ minWidth: 0 }}>
                    <span>{o.title}</span>
                    <span className="note">
                      {dMon(o.date)}, {DOC_LABEL[o.doc]}, {o.att?.length ? `${o.att.length} ${plural(o.att.length, 'вложение', 'вложения', 'вложений')}` : 'снимка нет'}
                    </span>
                  </span>
                  <span className="opright">
                    <span className="num">{rub(o.amount)}</span>
                    <button type="button" className="btn link" style={{ minHeight: 32, padding: 0, fontSize: 13, color: 'var(--own)' }} onClick={() => opAct.attach(o)}>
                      {o.att?.length ? 'Ещё снимок' : 'Добавить снимок'}
                    </button>
                  </span>
                </div>
                {!!o.att?.length && (
                  <div style={{ paddingLeft: 36 }}>
                    <Thumbs items={o.att} onOpen={opAct.view} />
                  </div>
                )}
              </div>
            ) : (
              <div key={o.id} style={{ display: 'grid', gridTemplateColumns: '24px minmax(0,1fr) auto', gap: 12, alignItems: 'center', minHeight: 56, padding: '6px 0', borderTop: '1px solid var(--line)' }}>
                <span className="warn">
                  <Icon name="alert" size={20} stroke={2} />
                </span>
                <span className="stack" style={{ minWidth: 0 }}>
                  <span>{o.title}</span>
                  <span className="note warn">{dMon(o.date)}, нет документа, из суточных</span>
                </span>
                <span className="opright">
                  <span className="num">{rub(o.amount)}</span>
                  <span style={{ display: 'flex', gap: 6 }}>
                    <button type="button" className="btn warn" style={{ minHeight: 32, padding: '0 8px', fontSize: 13, fontWeight: 400 }} onClick={() => opAct.attach(o)}>
                      Снимок
                    </button>
                    <button type="button" className="btn ghost" style={{ minHeight: 32, padding: '0 8px', fontSize: 13, fontWeight: 400 }} onClick={() => act.addDoc(o)}>
                      Документ есть
                    </button>
                  </span>
                </span>
              </div>
            ),
          )}
        </div>
      )}

      <div style={{ borderTop: '1px solid var(--line)', paddingTop: 12 }}>
        {!confirmDelete ? (
          <button type="button" className="btn link" onClick={() => setConfirmDelete(true)}>
            Удалить поездку
          </button>
        ) : (
          <div className="stack gap10">
            <p className="note warn">Удалить поездку {t.id} и все её операции? Отменить это нельзя.</p>
            <div className="wrap">
              <button type="button" className="btn warn" onClick={() => act.remove(t.id)}>
                Удалить
              </button>
              <button type="button" className="btn ghost" onClick={() => setConfirmDelete(false)}>
                Отмена
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
