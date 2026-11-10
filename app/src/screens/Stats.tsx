import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent, type RefObject } from 'react';
import { api, LOCKED, message, type MonthStat, type Snapshot, type StatSpan, type Stats } from '../api';
import { NB, addDays, dRange, fmt, plural, rub } from '../format';
import { CATS } from '../logic';
import { Icon, Seg } from '../ui';
import { MonthCard } from './Review';
import type { FinanceActions } from './Finance';

const NOM = ['январь', 'февраль', 'март', 'апрель', 'май', 'июнь', 'июль', 'август', 'сентябрь', 'октябрь', 'ноябрь', 'декабрь'];
const GEN = ['января', 'февраля', 'марта', 'апреля', 'мая', 'июня', 'июля', 'августа', 'сентября', 'октября', 'ноября', 'декабря'];
const PREP = ['январе', 'феврале', 'марте', 'апреле', 'мае', 'июне', 'июле', 'августе', 'сентябре', 'октябре', 'ноябре', 'декабре'];
const SHORT = ['янв', 'фев', 'мар', 'апр', 'май', 'июн', 'июл', 'авг', 'сен', 'окт', 'ноя', 'дек'];
const QUARTERS = ['I', 'II', 'III', 'IV'];

const cap = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);
/** «2026-11-07» или «2026-11» → [2026, 11]. */
const ym = (iso: string) => iso.split('-').map(Number) as [number, number];

/** «Ноябрь 2026», «IV квартал 2026», «2026». */
function periodTitle(span: StatSpan, from: string): string {
  const [y, m] = ym(from);
  if (span === 'year') return String(y);
  if (span === 'quarter') return `${QUARTERS[Math.floor((m - 1) / 3)]} квартал ${y}`;
  return `${cap(NOM[m - 1])} ${y}`;
}

/** После «за»: «ноябрь 2026», «IV квартал 2026», «2026 год». */
function periodWhat(span: StatSpan, from: string): string {
  const [y, m] = ym(from);
  if (span === 'year') return `${y} год`;
  if (span === 'quarter') return `${QUARTERS[Math.floor((m - 1) / 3)]} квартал ${y}`;
  return `${NOM[m - 1]} ${y}`;
}

/** С чем сравнение: «в октябре» или, пока период идёт, «за те же дни октября». */
function base(span: StatSpan, s: Stats): string {
  const [y, m] = ym(s.prev_from);
  if (span === 'year') return s.partial ? `за те же дни ${y} года` : `в ${y} году`;
  if (span === 'quarter') return s.partial ? 'за те же дни прошлого квартала' : 'в прошлом квартале';
  return s.partial ? `за те же дни ${GEN[m - 1]}` : `в ${PREP[m - 1]}`;
}

/** «На 12% больше, чем в октябре: 7 200 ₽», «В 3 раза больше…», «Столько же…». */
function compareText(span: StatSpan, s: Stats): string {
  const b = base(span, s);
  if (!s.prev_spent) return s.spent ? `${cap(b)} расходов не было` : '';
  const d = s.spent - s.prev_spent;
  if (d === 0) return `Столько же, сколько ${b}`;
  let how: string;
  if (s.spent >= 2 * s.prev_spent) {
    const ratio = s.spent / s.prev_spent;
    const r = ratio < 10 ? Math.round(ratio * 10) / 10 : Math.round(ratio);
    how = `В ${String(r).replace('.', ',')}${NB}${Number.isInteger(r) ? plural(r, 'раз', 'раза', 'раз') : 'раза'} больше`;
  } else {
    const pct = Math.round((Math.abs(d) / s.prev_spent) * 100);
    how = pct === 0 ? (d > 0 ? 'Чуть больше' : 'Чуть меньше') : `На ${pct}% ${d > 0 ? 'больше' : 'меньше'}`;
  }
  return `${how}, чем ${b}: ${rub(s.prev_spent)}`;
}

/** Компактная подпись оси: 0, 500, 1,5 тыс, 50 тыс, 1,2 млн. */
function compact(kop: number): string {
  const r = kop / 100;
  const one = (v: number) => String(Math.round(v * 10) / 10).replace('.', ',');
  if (r >= 1e6) return one(r / 1e6) + NB + 'млн';
  if (r >= 1e3) return one(r / 1e3) + NB + 'тыс';
  return String(Math.round(r));
}

/** Круглый шаг оси: 1, 2 или 5 × 10ⁿ. */
function niceStep(max: number, ticks: number): number {
  const raw = max / ticks;
  const p = Math.pow(10, Math.floor(Math.log10(raw)));
  const f = raw / p;
  return (f <= 1 ? 1 : f <= 2 ? 2 : f <= 5 ? 5 : 10) * p;
}

function useWidth<T extends HTMLElement>(): [RefObject<T | null>, number] {
  const ref = useRef<T>(null);
  const [w, setW] = useState(0);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const on = () => setW(el.clientWidth);
    on();
    const ro = new ResizeObserver(on);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return [ref, w];
}

/** Столбик: верх закруглён на 4px, низ прямой — стоит на базовой линии. */
function column(x: number, y: number, w: number, h: number): string {
  if (h <= 0.5) return '';
  const r = Math.min(4, w / 2, h);
  return `M${x},${y + h}V${y + r}Q${x},${y} ${x + r},${y}H${x + w - r}Q${x + w},${y} ${x + w},${y + r}V${y + h}Z`;
}

function monthLabel(month: string, short = false): string {
  const [y, m] = ym(month);
  return `${cap(short ? SHORT[m - 1] : NOM[m - 1])} ${y}`;
}

/** Доходы и расходы по месяцам: пара столбиков на месяц; наведение, касание и стрелки показывают суммы. */
function MonthsChart({ months }: { months: MonthStat[] }) {
  const [box, width] = useWidth<HTMLDivElement>();
  const [active, setActive] = useState(months.length - 1);
  const [table, setTable] = useState(false);
  const groups = useRef<(SVGGElement | null)[]>([]);
  // Сдвинулся период — снова последний месяц; новая операция в том же окне выбор не сбрасывает.
  const windowKey = months.map((m) => m.month).join();
  useEffect(() => setActive(months.length - 1), [windowKey, months.length]);
  const H = 184;
  const top = 10;
  const bottom = 24;
  const left = 50;
  const plotH = H - top - bottom;
  const plotW = Math.max(240, width - left - 2);
  const band = plotW / months.length;
  const bar = Math.max(3, Math.min(24, (band - 6) / 2 - 1));
  // Пустой год — шкала до 1 000 ₽, а не «0, 0, 0».
  const max = Math.max(100000, ...months.map((m) => Math.max(m.income, m.spent)));
  const step = niceStep(max, 4);
  const yMax = Math.ceil(max / step) * step;
  const ticks = Array.from({ length: Math.round(yMax / step) + 1 }, (_, i) => i * step);
  const y = (v: number) => top + plotH - (v / yMax) * plotH;
  const narrow = band < 30;
  const cur = months[Math.min(active, months.length - 1)];
  const empty = months.every((m) => !m.income && !m.spent);
  const move = (e: KeyboardEvent, i: number) => {
    const to = e.key === 'ArrowLeft' ? i - 1 : e.key === 'ArrowRight' ? i + 1 : e.key === 'Home' ? 0 : e.key === 'End' ? months.length - 1 : -1;
    if (to < 0 || to >= months.length) return;
    e.preventDefault();
    setActive(to);
    groups.current[to]?.focus();
  };
  return (
    <div className="stack gap10">
      <div className="legend" aria-hidden="true">
        <span>
          <i style={{ background: 'var(--serOwn)' }} />
          Доходы
        </span>
        <span>
          <i style={{ background: 'var(--serWork)' }} />
          Расходы
        </span>
      </div>
      <p className="readout" aria-live="polite">
        <span className="when">{monthLabel(cur.month)}:</span> доходы <b>{rub(cur.income)}</b> · расходы <b>{rub(cur.spent)}</b>
        {cur.saved !== 0 && (
          <>
            {' '}
            · {cur.saved > 0 ? 'отложено' : 'взято из накоплений'} <b>{rub(Math.abs(cur.saved))}</b>
          </>
        )}
      </p>
      <div ref={box} className="chartbox">
        {width > 0 && (
          <svg width={plotW + left + 2} height={H} role="group" aria-label={`Доходы и расходы по месяцам: ${monthLabel(months[0].month)} – ${monthLabel(months[months.length - 1].month)}`}>
            {ticks.map((t) => (
              <g key={t} aria-hidden="true">
                {t > 0 && <line x1={left} x2={left + plotW} y1={Math.round(y(t)) + 0.5} y2={Math.round(y(t)) + 0.5} className="grid" />}
                <text x={left - 8} y={y(t) + 4} textAnchor="end" className="tick">
                  {compact(t)}
                </text>
              </g>
            ))}
            {months.map((m, i) => {
              const x0 = left + i * band;
              const cx = x0 + band / 2;
              const mon = ym(m.month)[1];
              const on = i === active;
              return (
                <g
                  key={m.month}
                  ref={(el) => {
                    groups.current[i] = el;
                  }}
                  role="button"
                  tabIndex={on ? 0 : -1}
                  aria-pressed={on}
                  aria-label={`${monthLabel(m.month)}: доходы ${rub(m.income)}, расходы ${rub(m.spent)}`}
                  className={'mgroup' + (on ? ' on' : '')}
                  onPointerEnter={(e) => e.pointerType === 'mouse' && setActive(i)}
                  onClick={() => setActive(i)}
                  onFocus={() => setActive(i)}
                  onKeyDown={(e) => move(e, i)}
                >
                  <rect x={x0 + 1} y={top - 6} width={band - 2} height={plotH + 6} rx={6} className="band" />
                  <path d={column(cx - bar - 1, y(m.income), bar, top + plotH - y(m.income))} style={{ fill: 'var(--serOwn)' }} />
                  <path d={column(cx + 1, y(m.spent), bar, top + plotH - y(m.spent))} style={{ fill: 'var(--serWork)' }} />
                  <text x={cx} y={H - 7} textAnchor="middle" className={'tick' + (on ? ' strong' : '')}>
                    {narrow ? SHORT[mon - 1].charAt(0).toUpperCase() : SHORT[mon - 1]}
                  </text>
                </g>
              );
            })}
            <line x1={left} x2={left + plotW} y1={top + plotH + 0.5} y2={top + plotH + 0.5} className="axis" />
          </svg>
        )}
      </div>
      {empty && <p className="note">За эти месяцы доходов и расходов нет.</p>}
      <button type="button" className="btn link" style={{ alignSelf: 'flex-start', color: 'var(--own)', padding: 0 }} aria-expanded={table} onClick={() => setTable(!table)}>
        {table ? 'Скрыть таблицу' : 'Таблица по месяцам'}
      </button>
      {table && (
        <table className="stattable">
          <thead>
            <tr>
              <th scope="col">Месяц</th>
              <th scope="col">Доходы, ₽</th>
              <th scope="col">Расходы, ₽</th>
              <th scope="col">Отложено, ₽</th>
            </tr>
          </thead>
          <tbody>
            {months.map((m) => (
              <tr key={m.month}>
                <th scope="row">{monthLabel(m.month, true)}</th>
                <td>{fmt(m.income)}</td>
                <td>{fmt(m.spent)}</td>
                <td>{m.saved ? fmt(m.saved) : '—'}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

/** Расходы по категориям: одна сумма сверху, полоски в одном цвете, сравнение — словами. */
function Categories({ stats, span }: { stats: Stats; span: StatSpan }) {
  const shown = stats.cats.filter((c) => c.amount > 0);
  const gone = stats.cats.filter((c) => c.amount === 0 && c.prev > 0);
  const max = Math.max(1, ...shown.map((c) => c.amount));
  const cmp = compareText(span, stats);
  return (
    <div className="stack">
      <p className="statbig">{rub(stats.spent)}</p>
      <p className="note" style={{ marginBottom: 10 }}>
        {cmp}
        {cmp && stats.income ? ' · ' : ''}
        {stats.income ? `доходы ${rub(stats.income)}` : ''}
      </p>
      {shown.map((c) => {
        const cat = CATS[c.cat] || CATS.other;
        const share = stats.spent ? (c.amount / stats.spent) * 100 : 0;
        const d = c.amount - c.prev;
        const vs = c.prev === 0 ? 'раньше не было' : d === 0 ? 'столько же' : `на ${rub(Math.abs(d))} ${d > 0 ? 'больше' : 'меньше'}`;
        return (
          <div key={c.cat} className="budrow statcat">
            <span className="opicon own" aria-hidden="true">
              <Icon name={cat.i} size={18} />
            </span>
            <span className="stack" style={{ minWidth: 0, gap: 5 }}>
              <span className="between">
                <span className="optitle">{cat.n}</span>
                <span className="num" style={{ flex: 'none' }}>
                  {rub(c.amount)}
                </span>
              </span>
              <span className="hbar" aria-hidden="true">
                <i style={{ width: `${Math.max(1, (c.amount / max) * 100)}%` }} />
              </span>
              <span className="note">
                {share < 1 ? 'меньше 1%' : `${Math.round(share)}%`} расходов · {vs}
              </span>
            </span>
          </div>
        );
      })}
      {gone.length > 0 && (
        <p className="note" style={{ paddingTop: 10, borderTop: '1px solid var(--line)' }}>
          {cap(base(span, stats))} были ещё: {gone.map((c) => `${(CATS[c.cat] || CATS.other).n.toLowerCase()} — ${rub(c.prev)}`).join(', ')}.
        </p>
      )}
      {shown.length === 0 && <p className="empty">Расходов за этот период нет.</p>}
      <p className="note" style={{ marginTop: 10 }}>
        Личные и семейные расходы. Деньги компании, переводы между счетами и долги сюда не входят.
      </p>
    </div>
  );
}

/** Командировки за год: итоги и список поездок. */
function Trips({ stats, openTrip }: { stats: Stats; openTrip: (id: string) => void }) {
  const t = stats.trips;
  if (!t.count) return <p className="empty">В {t.year} году поездок нет.</p>;
  const figs: { label: string; value: string; warn?: boolean }[] = [
    { label: plural(t.count, 'Поездка', 'Поездки', 'Поездок'), value: String(t.count) },
    { label: 'Дней в поездках', value: String(t.days) },
    { label: 'Суточные', value: rub(t.per_diem) },
    { label: 'Осталось от суточных', value: rub(t.left) },
    { label: 'Расходы с документами', value: rub(t.confirmed) },
    { label: 'Долг перед компанией', value: rub(t.debt), warn: t.debt > 0 },
  ];
  return (
    <div className="stack">
      <div className="figures statfigs" style={{ marginBottom: 12 }}>
        {figs.map((f) => (
          <div key={f.label} className="figure">
            <span className="lab">{f.label}</span>
            <span className={'val' + (f.warn ? ' warn' : '')}>{f.value}</span>
          </div>
        ))}
      </div>
      {t.items.map((it) => (
        <button key={it.id} type="button" className="triprow" onClick={() => openTrip(it.id)}>
          <span className="between">
            <span style={{ fontWeight: 500 }}>
              {it.id} {it.city}
            </span>
            <span className="num" style={{ flex: 'none' }}>
              {rub(it.per_diem)}
            </span>
          </span>
          <span className="between">
            <span className="note">
              {dRange(it.start, it.end)}, {it.days}
              {NB}
              {plural(it.days, 'день', 'дня', 'дней')}
              {it.finished ? '' : ', ещё идёт'}
            </span>
            <span className="note" style={{ flex: 'none' }}>
              суточные
            </span>
          </span>
          <span className={'note' + (it.debt > 0 ? ' warn' : '')}>
            {it.debt > 0 ? `Долг перед компанией ${rub(it.debt)}` : `Осталось от суточных ${rub(it.left)}`}
            {it.confirmed ? ` · с документами ${rub(it.confirmed)}` : ''}
          </span>
        </button>
      ))}
    </div>
  );
}

/** «Статистика» в «Финансах»: месяц, квартал или год. */
export default function StatsScreen({ snap, act, mobile, openTrip }: { snap: Snapshot; act: FinanceActions; mobile: boolean; openTrip: (id: string) => void }) {
  const [span, setSpan] = useState<StatSpan>('month');
  const [anchor, setAnchor] = useState(snap.today);
  // Цифры на экране и период, за который они посчитаны: пока грузится другой, прежние — бледнее.
  const [shown, setShown] = useState<{ stats: Stats; span: StatSpan; key: string } | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    api.stats(span, anchor).then(
      (s) => {
        if (!alive) return;
        setShown({ stats: s, span, key: span + ':' + anchor });
        setError(null);
      },
      (e) => {
        if (!alive) return;
        if (message(e) === LOCKED) act.locked();
        else setError(message(e));
      },
    );
    return () => {
      alive = false;
    };
    // Новая операция меняет сводку — статистика пересчитывается.
  }, [span, anchor, snap]);
  const exportXlsx = async () => {
    setSaving(true);
    try {
      const path = await api.exportXlsx(span, anchor);
      act.flash('Файл Excel сохранён: ' + path, true);
    } catch (e) {
      if (message(e) === LOCKED) act.locked();
      else act.flash(message(e));
    } finally {
      setSaving(false);
    }
  };
  const stats = shown?.stats ?? null;
  const at = shown?.span ?? span;
  const busy = !shown || shown.key !== span + ':' + anchor;
  const canNext = !!stats && addDays(stats.to, 1) <= snap.today;
  return (
    <div className="stack gap26" style={{ marginTop: 10 }}>
      <div className="stack gap10">
        <Seg
          label="Период"
          value={span}
          onPick={setSpan}
          items={[
            { id: 'month', label: 'Месяц' },
            { id: 'quarter', label: 'Квартал' },
            { id: 'year', label: 'Год' },
          ]}
        />
        <div className="between" style={{ alignItems: 'center' }}>
          <button type="button" className="iconbtn" aria-label="Предыдущий период" disabled={!stats} onClick={() => stats && setAnchor(addDays(stats.from, -1))}>
            <Icon name="left" size={22} stroke={2} />
          </button>
          <span className="stack" style={{ alignItems: 'center' }}>
            <span className="h2" aria-live="polite">
              {stats ? periodTitle(at, stats.from) : NB}
            </span>
            {stats && at === 'quarter' && <span className="note">{dRange(stats.from, stats.to)}</span>}
          </span>
          <button type="button" className="iconbtn" aria-label="Следующий период" disabled={!canNext} onClick={() => stats && setAnchor(addDays(stats.to, 1))}>
            <Icon name="right" size={22} stroke={2} />
          </button>
        </div>
      </div>
      {error && <p className="error">{error}</p>}
      {stats && at === 'month' && stats.from <= snap.today && snap.today <= stats.to && <MonthCard snap={snap} locked={act.locked} />}
      {stats && (
        <div className="stack gap26" style={{ opacity: busy ? 0.6 : 1, transition: 'opacity .15s' }}>
          <section className="stack">
            <h2 className="h2" style={{ marginBottom: 6 }}>
              Расходы по категориям
            </h2>
            <Categories stats={stats} span={at} />
          </section>
          <section className="stack">
            <h2 className="h2" style={{ marginBottom: 10 }}>
              Доходы и расходы по месяцам
            </h2>
            <MonthsChart months={stats.months} />
          </section>
          <section className="stack">
            <h2 className="h2" style={{ marginBottom: 10 }}>
              Командировки за {stats.trips.year} год
            </h2>
            <Trips stats={stats} openTrip={openTrip} />
          </section>
          {!mobile && (
            <section className="stack gap10">
              <button type="button" className="btn ghost" style={{ alignSelf: 'flex-start', display: 'flex', alignItems: 'center', gap: 8 }} disabled={saving || busy} onClick={exportXlsx}>
                <Icon name="download" size={18} />
                Выгрузить в Excel
              </button>
              <p className="note">
                Операции за {periodWhat(at, stats.from)}, итоги по категориям и по месяцам, командировки за {stats.trips.year} год — файлом в «Загрузки».
              </p>
            </section>
          )}
        </div>
      )}
    </div>
  );
}
