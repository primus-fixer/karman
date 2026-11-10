import { useEffect, useState } from 'react';
import { api, LOCKED, message, type Finding, type Insights, type MonthSummary, type Review, type Snapshot } from '../api';
import { aiOn, useAi } from '../aiState';
import { NB, dMon, rub } from '../format';
import { Icon } from '../ui';
import { costText } from './Ai';

const LEVEL: Record<Finding['level'], { mark: string; cls: string; label: string }> = {
  problem: { mark: '!', cls: 'warn', label: 'исправить' },
  check: { mark: '?', cls: '', label: 'проверить' },
  fine: { mark: '✓', cls: 'faint', label: 'в порядке' },
};

/** Находки Claude списком: сначала что исправить, потом что проверить. */
export function Findings({ summary, items }: { summary: string; items: Finding[] }) {
  const order = { problem: 0, check: 1, fine: 2 };
  const sorted = [...items].sort((a, b) => order[a.level] - order[b.level]);
  return (
    <div className="stack gap6">
      {summary && <p style={{ margin: 0 }}>{summary}</p>}
      <ul className="findings">
        {sorted.map((f, i) => (
          <li key={i} className={LEVEL[f.level].cls}>
            <span className="fmark" aria-label={LEVEL[f.level].label}>
              {LEVEL[f.level].mark}
            </span>
            <span>{f.text}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}

/** «Проверить с Claude» в отчёте поездки: чего не хватает перед сдачей. */
export function TripCheck({ tripId, locked }: { tripId: string; locked: () => void }) {
  const ai = useAi();
  const [busy, setBusy] = useState(false);
  const [res, setRes] = useState<Review | null>(null);
  const [error, setError] = useState<string | null>(null);
  if (!aiOn(ai, 'review')) return null;
  const check = async () => {
    setBusy(true);
    setError(null);
    try {
      setRes(await api.aiTripCheck(tripId));
    } catch (e) {
      if (message(e) === LOCKED) locked();
      else setError(message(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="card stack gap10" aria-label="Проверка перед сдачей">
      <div className="between" style={{ alignItems: 'center', gap: 8 }}>
        <span style={{ display: 'flex', alignItems: 'center', gap: 6, fontWeight: 500 }}>
          <Icon name="spark" size={18} />
          Проверка перед сдачей
        </span>
        <button type="button" className="btn ghost small" disabled={busy} onClick={check}>
          {busy ? 'Claude проверяет…' : res ? 'Проверить ещё раз' : 'Проверить с Claude'}
        </button>
      </div>
      {!res && !busy && !error && <p className="note">Claude посмотрит отчёт поездки: документы, статьи против заявки, суточные и долг — и скажет, что поправить до сдачи.</p>}
      {busy && <p className="note">Обычно до минуты.</p>}
      {res && <Findings summary={res.summary} items={res.items} />}
      {res && res.usd > 0 && <p className="note faint">Запрос: {costText(res.usd)}</p>}
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}

const ddmm = (iso: string) => iso.slice(8, 10) + '.' + iso.slice(5, 7);

/** Сводка месяца: «Карман» считает сам, Claude по кнопке пересказывает. */
export function MonthCard({ snap, locked }: { snap: Snapshot; locked: () => void }) {
  const ai = useAi();
  const [ins, setIns] = useState<Insights | null>(null);
  const [sum, setSum] = useState<MonthSummary | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    api.monthInsights().then(
      (i) => alive && setIns(i),
      (e) => alive && (message(e) === LOCKED ? locked() : setError(message(e))),
    );
    return () => {
      alive = false;
    };
  }, [snap, locked]);
  if (!ins) return error ? <p className="error">{error}</p> : null;
  const f = ins.forecast;
  const tell = async () => {
    setBusy(true);
    setError(null);
    try {
      setSum(await api.aiMonthSummary());
    } catch (e) {
      if (message(e) === LOCKED) locked();
      else setError(message(e));
    } finally {
      setBusy(false);
    }
  };
  const diff = ins.spent - ins.usual_to_date;
  return (
    <section className="card stack gap10" aria-label="Сводка месяца">
      <h2 className="h2">Сводка месяца</h2>
      <p style={{ margin: 0 }}>
        Потрачено {rub(ins.spent)} за {ins.days}
        {NB}дн.
        {ins.usual_to_date > 0 && (
          <span className={diff > 0 ? 'warn' : 'faint'}>
            {' '}
            — {diff > 0 ? 'больше' : 'меньше'} обычного на {rub(Math.abs(diff))}
          </span>
        )}
      </p>
      {ins.unusual.length > 0 && (
        <div className="stack gap6">
          <span className="note">Больше обычного</span>
          {ins.unusual.map((c) => (
            <span key={c.cat}>
              {c.name}: {rub(c.spent)} <span className="note">(обычно {rub(c.usual)} в месяц)</span>
            </span>
          ))}
        </div>
      )}
      {ins.big.length > 0 && (
        <div className="stack gap6">
          <span className="note">Крупные покупки</span>
          {ins.big.map((b) => (
            <span key={b.id}>
              {b.title}: {rub(b.amount)} <span className="note">{dMon(b.date)}</span>
            </span>
          ))}
        </div>
      )}
      {ins.recurring.length > 0 && (
        <div className="stack gap6">
          <span className="note">Подписки и регулярные траты</span>
          {ins.recurring.map((r) => (
            <span key={r.title}>
              {r.title}: {rub(r.amount)} в месяц <span className="note">· следующая ≈ {ddmm(r.next)}</span>
              {!r.in_plans && <span className="note warn"> · не в регулярных платежах</span>}
            </span>
          ))}
        </div>
      )}
      <div className="stack gap6">
        <span className="note">Прогноз «Моих денег» на конец месяца</span>
        <span>
          ≈ {rub(f.end)}{' '}
          <span className="note">
            (сейчас {rub(f.now)}
            {f.income > 0 ? `, ожидаются доходы +${rub(f.income)}` : ''}
            {f.planned > 0 ? `, регулярные платежи −${rub(f.planned)}` : ''}
            {f.spending > 0 ? `, обычные траты −${rub(f.spending)} за ${f.days_left}${NB}дн.` : ''})
          </span>
        </span>
      </div>
      {aiOn(ai, 'review') && (
        <div className="stack gap10" style={{ borderTop: '1px solid var(--line)', paddingTop: 10 }}>
          <button type="button" className="btn ghost small" style={{ alignSelf: 'flex-start', display: 'flex', alignItems: 'center', gap: 6 }} disabled={busy} onClick={tell}>
            <Icon name="spark" size={16} />
            {busy ? 'Claude пишет сводку…' : sum ? 'Пересказать ещё раз' : 'Пересказать с Claude'}
          </button>
          {sum && <Findings summary={sum.summary} items={sum.items} />}
          {sum && sum.usd > 0 && <p className="note faint">Запрос: {costText(sum.usd)}</p>}
        </div>
      )}
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}
