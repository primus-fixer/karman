import { useState } from 'react';
import type { Check, Op, Snapshot } from './api';
import { ICON } from './icons';
import { opView, rail } from './logic';

export function Icon({ name, size = 20, stroke = 1.75, d }: { name?: string; size?: number; stroke?: number; d?: string }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={stroke} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d={d || ICON[name || 'other']} />
    </svg>
  );
}

export function Seg<T extends string>({ items, value, onPick, label }: { items: { id: T; label: string; tone?: 'own' | 'work' }[]; value: T; onPick: (id: T) => void; label: string }) {
  return (
    <div className="seg" role="group" aria-label={label} style={{ gridTemplateColumns: `repeat(${items.length}, minmax(0, 1fr))` }}>
      {items.map((it) => (
        <button key={it.id} type="button" aria-pressed={it.id === value} className={(it.id === value ? 'on ' : '') + (it.tone || '')} onClick={() => onPick(it.id)}>
          {it.label}
        </button>
      ))}
    </div>
  );
}

/** Маленькая полоска статуса в списке поездок. */
export function Dots({ statusIdx, label }: { statusIdx: number; label: string }) {
  const steps = rail(statusIdx);
  return (
    <span className="dots" role="img" aria-label={label}>
      {steps.map((s, i) => (
        <span key={i} style={{ display: 'inline-flex', alignItems: 'center' }}>
          <i className={s.done || s.current ? 'on' : ''} />
          {i < steps.length - 1 && <b className={i < statusIdx ? 'on' : ''} />}
        </span>
      ))}
    </span>
  );
}

export function Rail({ statusIdx }: { statusIdx: number }) {
  const steps = rail(statusIdx);
  return (
    <ol className={'rail' + (statusIdx === 5 ? ' closed' : '')} aria-label="Статус поездки">
      {steps.map((s, i) => (
        <li key={s.label} aria-current={s.current ? 'step' : undefined}>
          <span className="track">
            <span className={'sq' + (s.done || s.current ? ' on' : '') + (s.current && statusIdx < 5 ? ' cur' : '')} />
            {i < steps.length - 1 && <span className={'bar' + (i < statusIdx ? ' on' : '')} />}
          </span>
          <span className={'lab' + (s.current ? ' cur' : '')}>{s.label}</span>
        </li>
      ))}
    </ol>
  );
}

export function Checks({ items }: { items: Check[] }) {
  return (
    <ul className="checks">
      {items.map((c, i) => (
        <li key={i}>
          <svg className={c.ok ? 'pos' : 'warn'} width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" aria-label={c.ok ? 'Да' : 'Нет'}>
            <path d={c.ok ? 'M20 6 9 17l-5-5' : 'M18 6 6 18M6 6l12 12'} />
          </svg>
          <span>{c.text}</span>
        </li>
      ))}
    </ul>
  );
}

export function ErrorText({ text }: { text: string | null }) {
  return text ? (
    <p className="error" role="alert">
      {text}
    </p>
  ) : null;
}

/** Строка операции, раскрывается по нажатию. */
export type OpActions = { addDoc: (op: Op) => void; remove: (op: Op) => void; edit: (op: Op) => void };

/** Эти операции можно поправить в окне ввода; возврат компании и платежи по долгам — только удалить. */
export const EDITABLE = new Set(['work', 'food', 'personal', 'income', 'advance', 'reimbursement']);

export function OpRow({ snap, op, act, dateSub }: { snap: Snapshot; op: Op; act: OpActions; dateSub?: boolean }) {
  const [open, setOpen] = useState(false);
  const v = opView(snap, op);
  return (
    <div className="op">
      <button type="button" aria-expanded={open} onClick={() => setOpen(!open)}>
        <span className={'opicon' + (v.own ? ' own' : '')}>
          <Icon d={v.icon} size={19} />
        </span>
        <span className="stack" style={{ minWidth: 0 }}>
          <span className="optitle">{v.title}</span>
          <span className="opsub">{dateSub ? v.dateSub : v.sub}</span>
        </span>
        <span className="opright">
          <span className={'amount' + (v.inc ? ' in' : '')}>{v.amount}</span>
          <span style={{ display: 'flex', gap: 4 }}>
            {v.noDoc && <span className="tag warn">без чека</span>}
            {v.chip && <span className={'tag' + (v.chipTagged ? ' grey' : '')}>{v.chip}</span>}
          </span>
        </span>
      </button>
      {open && (
        <div className="opdetails">
          {v.details.map((d) => (
            <div className="d" key={d.label}>
              <span>{d.label}</span>
              <span style={{ textAlign: 'right' }}>{d.value}</span>
            </div>
          ))}
          <div className="wrap">
            {v.noDoc && (
              <button type="button" className="btn warn" onClick={() => act.addDoc(op)}>
                Документ есть
              </button>
            )}
            {EDITABLE.has(op.kind) && (
              <button
                type="button"
                className="btn ghost"
                onClick={() => {
                  setOpen(false);
                  act.edit(op);
                }}
              >
                Изменить
              </button>
            )}
            <button type="button" className="btn link" onClick={() => act.remove(op)}>
              Удалить
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
