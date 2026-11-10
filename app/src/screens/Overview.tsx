import { useState } from 'react';
import type { Snapshot } from '../api';
import { NB, fmt, plural, rub, weekdayLabel } from '../format';
import { CATS, isSpend, statusIndex, tripRow } from '../logic';
import { Dots, Icon, OpRow, type OpActions } from '../ui';
import type { Nav } from '../App';
import { DueRow, type FinanceActions, type FinView } from './Finance';
import { Bar, leftText } from './Budget';
import { daysSince, lastBackup } from './Backup';

export function Figures({ snap, nav, big = true }: { snap: Snapshot; nav: Nav; big?: boolean }) {
  const items: { label: string; value: string; note?: string; mark: string; go: () => void }[] = [];
  if (snap.held > 0) items.push({ label: 'Подотчётные', value: rub(snap.held), mark: 'mark fill', go: () => nav.go('trips') });
  if (snap.owed > 0) items.push({ label: 'Компания должна', value: rub(snap.owed), mark: 'mark', go: () => nav.go('trips') });
  if (snap.due > 0)
    items.push({ label: 'Вернуть компании', value: rub(snap.due), note: snap.debt_due > 0 ? 'в т. ч. долг ' + rub(snap.debt_due) : '', mark: 'mark due', go: () => nav.go('trips') });
  if (!items.length) return null;
  return (
    <div className="figures">
      {items.map((f) => (
        <button key={f.label} type="button" className="figure" onClick={f.go}>
          <span className="lab">
            <span className={f.mark} />
            {f.label}
          </span>
          <span className="val" style={big ? undefined : { fontSize: 22, lineHeight: '28px' }}>
            {f.value}
          </span>
          {f.note && <span className="note warn">{f.note}</span>}
        </button>
      ))}
    </div>
  );
}

export function TripList({ snap, nav, closed = false }: { snap: Snapshot; nav: Nav; closed?: boolean }) {
  const rows = snap.trips.map((tv) => tripRow(tv)).filter((r) => r.closed === closed);
  return (
    <div className={closed ? 'closedtrips' : ''}>
      {rows.map((r) =>
        closed ? (
          <button key={r.tv.trip.id} type="button" className="triprow" style={{ flexDirection: 'row', justifyContent: 'space-between', alignItems: 'center' }} onClick={() => nav.openTrip(r.tv.trip.id)}>
            <span className="stack">
              <span style={{ fontWeight: 500 }}>{r.name}</span>
              <span className="note">{r.dates}</span>
            </span>
            <span className="note">{r.balanceNote}</span>
          </button>
        ) : (
          <button key={r.tv.trip.id} type="button" className="triprow" onClick={() => nav.openTrip(r.tv.trip.id)}>
            <span className="between">
              <span style={{ fontWeight: 500 }}>{r.name}</span>
              <span className={'num' + (r.balanceWarn ? ' warn' : '')} style={{ flex: 'none', fontWeight: 500 }}>
                {r.balanceText}
              </span>
            </span>
            <span className="between" style={{ alignItems: 'center' }}>
              <Dots statusIdx={r.statusIdx} label={r.statusText} />
              <span className="note" style={{ textAlign: 'right' }}>
                {r.balanceNote}
              </span>
            </span>
            <span className="note">
              {r.dates}. {r.statusNote}
            </span>
            {r.docWarn && (
              <span className="note warn" style={{ display: 'flex', gap: 6, alignItems: 'center' }}>
                <Icon name="alert" size={14} stroke={2} />
                {r.docWarn}
              </span>
            )}
          </button>
        ),
      )}
    </div>
  );
}

/**
 * В настройки, к разделу «Резервная копия». Разделы выше (облако, Claude, ИИ) догружаются и становятся
 * выше — полторы секунды держим раздел на месте, пока человек сам не прокрутит.
 */
function goBackup(nav: Nav) {
  nav.go('settings');
  let stop = false;
  let want: number | null = null;
  const cancel = () => (stop = true);
  const events = ['wheel', 'touchstart', 'keydown', 'mousedown'] as const;
  events.forEach((e) => window.addEventListener(e, cancel, { passive: true }));
  const t0 = Date.now();
  const tick = () => {
    const el = document.getElementById('backup');
    if (el && !stop) {
      const top = el.getBoundingClientRect().top;
      if (want === null || Math.abs(top - want) > 2) {
        el.scrollIntoView({ block: 'start' });
        want = el.getBoundingClientRect().top;
      }
    }
    if (!stop && Date.now() - t0 < 1500) window.requestAnimationFrame(tick);
    else events.forEach((e) => window.removeEventListener(e, cancel));
  };
  window.setTimeout(tick, 80);
}

export default function Overview({ snap, nav, act, fin, openFinance, mobile, ask }: { snap: Snapshot; nav: Nav; act: OpActions; fin: FinanceActions; openFinance: (v: FinView) => void; mobile: boolean; ask?: () => void }) {
  const [accountsOpen, setAccountsOpen] = useState(false);
  const mine = fmt(snap.mine);
  const comma = mine.indexOf(',');
  const todayOps = snap.ops.filter((o) => o.date === snap.today);
  const todaySpend = todayOps.filter(isSpend).reduce((s, o) => s + o.amount, 0);
  const openTrips = snap.trips.filter((t) => statusIndex(t.trip.status) < 5);
  const empty = snap.trips.length === 0 && snap.ops.length === 0;
  const soon = snap.upcoming.filter((u) => u.soon);
  // Бюджеты, где задан хоть один лимит.
  const budgets = snap.budgets.filter((b) => b.total || b.cats.some((c) => c.limit != null));
  // Копий не было или они давно не делались — напомнить (когда записей уже набралось).
  const last = lastBackup(snap);
  const backupDays = last ? daysSince(last.at) : null;
  const remindBackup = !empty && snap.ops.length >= 10 && (backupDays == null || backupDays > 14);
  const debts: string[] = [];
  if (snap.i_owe > 0) debts.push('я должен ' + rub(snap.i_owe));
  if (snap.owed_to_me > 0) debts.push('мне должны ' + rub(snap.owed_to_me));
  return (
    <div className="stack gap26">
      <p className="note" style={{ fontSize: 15, lineHeight: '21px', margin: '-4px 0 -14px' }}>
        {weekdayLabel(snap.today)}
      </p>
      {ask && (
        <button type="button" className="askbar" onClick={ask}>
          <Icon name="spark" size={18} />
          <span>Спросить Claude о деньгах и поездках</span>
        </button>
      )}
      <section className="stack gap16">
        <div>
          <h1 className="note" style={{ fontSize: 15, lineHeight: '20px', fontWeight: 500 }}>
            Мои деньги
          </h1>
          <p className="big">
            {comma < 0 ? mine : mine.slice(0, comma)}
            <small>
              {comma < 0 ? '' : mine.slice(comma)}
              {NB}₽
            </small>
          </p>
        </div>
        <Figures snap={snap} nav={nav} />
        <div>
          <button type="button" className="collapse" aria-expanded={accountsOpen} onClick={() => setAccountsOpen(!accountsOpen)}>
            <span>На счетах</span>
            <span style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
              <span className="num" style={{ color: 'var(--ink)' }}>
                {rub(snap.total)}
              </span>
              <Icon name={accountsOpen ? 'up' : 'down'} size={18} stroke={2} />
            </span>
          </button>
          {accountsOpen && (
            <div className="stack" style={{ paddingBottom: 6 }}>
              {snap.accounts
                .filter((a) => !a.account.archived)
                .map((a) => (
                  <div key={a.account.id} className="between" style={{ padding: '5px 0' }}>
                    <span className="note" style={{ fontSize: 15 }}>
                      {a.account.name}
                    </span>
                    <span className="num" style={{ textAlign: 'right' }}>
                      {rub(a.balance, a.account.currency)}
                      {a.account.currency !== 'RUB' && <span className="note faint" style={{ display: 'block' }}>≈{NB + rub(a.balance_rub)}</span>}
                    </span>
                  </div>
                ))}
              <button type="button" className="btn link" style={{ alignSelf: 'flex-start', color: 'var(--own)', fontWeight: 500, padding: 0 }} onClick={() => nav.go('settings')}>
                Остатки и счета — в настройках
              </button>
            </div>
          )}
          {snap.saved > 0 && (
            <button type="button" className="collapse" onClick={() => openFinance('save')}>
              <span>
                Отложено
                {snap.saved_in_mine && <span className="note faint"> — входит в «Мои деньги»</span>}
              </span>
              <span className="num" style={{ color: 'var(--ink)' }}>
                {rub(snap.saved)}
              </span>
            </button>
          )}
          {debts.length > 0 && (
            <button type="button" className="collapse" onClick={() => openFinance('pay')}>
              <span>Долги</span>
              <span className="num" style={{ color: 'var(--ink)', textAlign: 'right', padding: '6px 0' }}>
                {debts.map((t) => (
                  <span key={t} style={{ display: 'block' }}>
                    {t}
                  </span>
                ))}
              </span>
            </button>
          )}
        </div>
      </section>

      {budgets.length > 0 && (
        <section className="stack">
          <div className="between" style={{ marginBottom: 8 }}>
            <h2 className="h2">Бюджет на месяц</h2>
            <button type="button" className="btn link" style={{ color: 'var(--own)', padding: 0 }} onClick={() => openFinance('budget')}>
              Подробнее
            </button>
          </div>
          {budgets.map((b) => {
            const over = b.cats.filter((c) => c.limit != null && c.spent > c.limit);
            return (
              <button key={b.id} type="button" className="budrow" style={{ width: '100%', textAlign: 'left', background: 'none', border: 0, borderTop: '1px solid var(--line)', color: 'inherit', font: 'inherit' }} onClick={() => openFinance('budget')}>
                <span className="stack" style={{ minWidth: 0, gap: 4, width: '100%' }}>
                  <span className="between">
                    <span>{b.id === 'family' ? 'Семейный' : 'Личный'}</span>
                    <span className="num">
                      {rub(b.spent)}
                      {b.total ? ' из ' + rub(b.total) : ''}
                    </span>
                  </span>
                  <Bar value={b.spent} limit={b.total} />
                  {b.total ? <span className={'note' + (b.spent > b.total ? ' warn' : '')}>{leftText(b.spent, b.total)}</span> : null}
                  {over.length > 0 && <span className="note warn">Сверх лимита: {over.map((c) => (CATS[c.cat] || CATS.other).n.toLowerCase()).join(', ')}</span>}
                </span>
              </button>
            );
          })}
        </section>
      )}

      {soon.length > 0 && (
        <section className="stack">
          <div className="between" style={{ marginBottom: 8 }}>
            <h2 className="h2">Скоро платежи</h2>
            <button type="button" className="btn link" style={{ color: 'var(--own)', padding: 0 }} onClick={() => openFinance('pay')}>
              Все в финансах
            </button>
          </div>
          {soon.slice(0, 3).map((u) => (
            <DueRow key={u.kind + u.id + u.date} snap={snap} due={u} act={fin} compact />
          ))}
        </section>
      )}

      {remindBackup && (
        <section className="card stack gap6 backupnote">
          <div className="between" style={{ alignItems: 'center' }}>
            <span style={{ fontWeight: 500 }}>{backupDays == null ? 'Резервных копий ещё не было' : `Последняя копия — ${backupDays}${NB}${plural(backupDays, 'день', 'дня', 'дней')} назад`}</span>
            <button type="button" className="btn ghost small" style={{ flex: 'none' }} onClick={() => goBackup(nav)}>
              {mobile ? 'Подробнее' : 'Настроить'}
            </button>
          </div>
          <span className="note">
            {mobile
              ? backupDays == null
                ? 'Их делает «Карман» на компьютере: задайте там пароль копии.'
                : 'Откройте «Карман» на компьютере и свяжите устройства по Wi-Fi.'
              : backupDays == null
                ? 'Задайте пароль копии — дальше копии будут делаться сами.'
                : 'Проверьте автокопии в настройках.'}
          </span>
        </section>
      )}

      {empty && (
        <section className="card stack gap10">
          <h2 className="h2">С чего начать</h2>
          <p className="note">
            Заведите поездку по заявке из 1С — аванс, суточные и отчёт посчитаются сами. Личные расходы можно записывать сразу кнопкой «+».
            Остатки по счетам укажите в настройках.
          </p>
          <div className="wrap">
            <button type="button" className="btn work" onClick={() => nav.newTrip()}>
              Новая поездка
            </button>
            <button type="button" className="btn ghost" onClick={() => nav.openEntry()}>
              Записать расход
            </button>
          </div>
          <p className="note">Уже пользовались «Карманом»? Восстановите данные из резервной копии или свяжите это устройство с другим по Wi-Fi.</p>
          <button type="button" className="btn link" style={{ alignSelf: 'flex-start', color: 'var(--own)', padding: 0, minHeight: 32 }} onClick={() => goBackup(nav)}>
            Восстановить из копии
          </button>
        </section>
      )}

      {openTrips.length > 0 && (
        <section className="stack">
          <h2 className="h2" style={{ marginBottom: 8 }}>
            Открытые поездки
          </h2>
          <TripList snap={snap} nav={nav} />
        </section>
      )}

      {todayOps.length > 0 && (
        <section className="stack">
          <div className="between" style={{ marginBottom: 8 }}>
            <h2 className="h2">Сегодня</h2>
            <span className="num note" style={{ fontSize: 15 }}>
              {todaySpend ? '−' + rub(todaySpend) : ''}
            </span>
          </div>
          {todayOps.map((o) => (
            <OpRow key={o.id} snap={snap} op={o} act={act} />
          ))}
        </section>
      )}
    </div>
  );
}
