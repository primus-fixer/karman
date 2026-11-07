import { useEffect, useState } from 'react';
import type { Attachment, Check, Op, Snapshot } from './api';
import { blobUrl, sizeText } from './media';
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
export type OpActions = {
  addDoc: (op: Op) => void;
  remove: (op: Op) => void;
  edit: (op: Op) => void;
  /** Прикрепить снимок или файл к записанной операции. */
  attach: (op: Op) => void;
  view: (a: Attachment) => void;
};

function Thumb({ att, onOpen, onRemove }: { att: Attachment; onOpen?: (a: Attachment) => void; onRemove?: (id: string) => void }) {
  const isImage = att.mime.startsWith('image/');
  // undefined — грузится, null — снимок ещё не пришёл с другого устройства.
  const [url, setUrl] = useState<string | null | undefined>(isImage ? undefined : null);
  useEffect(() => {
    if (!isImage) return;
    let alive = true;
    blobUrl(att.id).then(
      (u) => alive && setUrl(u),
      () => alive && setUrl(null),
    );
    return () => {
      alive = false;
    };
  }, [att.id, isImage]);
  const missing = isImage && url === null;
  return (
    <span className="thumb">
      <button
        type="button"
        className="thumbimg"
        aria-label={missing ? `${att.name}: ещё не пришёл с другого устройства` : `Открыть ${att.name}`}
        title={missing ? 'Снимок ещё не пришёл с другого устройства — свяжите устройства по Wi-Fi' : `${att.name}, ${sizeText(att.size)}`}
        disabled={!onOpen || missing}
        onClick={() => onOpen?.(att)}
      >
        {url ? (
          <img src={url} alt="" />
        ) : (
          <span className="thumbicon">
            <Icon name={isImage ? 'camera' : 'file'} size={20} />
            <small>{isImage ? (missing ? 'не пришёл' : '…') : 'PDF'}</small>
          </span>
        )}
      </button>
      {onRemove && (
        <button type="button" className="thumbx" aria-label={`Убрать ${att.name}`} onClick={() => onRemove(att.id)}>
          <Icon name="x" size={14} stroke={2.4} />
        </button>
      )}
    </span>
  );
}

/** Миниатюры снимков и файлов чека. */
export function Thumbs({ items, onOpen, onRemove }: { items: Attachment[]; onOpen?: (a: Attachment) => void; onRemove?: (id: string) => void }) {
  return (
    <div className="thumbs">
      {items.map((a) => (
        <Thumb key={a.id} att={a} onOpen={onOpen} onRemove={onRemove} />
      ))}
    </div>
  );
}

/** Просмотр снимка или PDF во весь экран. */
export function Viewer({ att, pdfInline, onClose }: { att: Attachment; pdfInline: boolean; onClose: () => void }) {
  const [url, setUrl] = useState<string | null | undefined>(undefined);
  useEffect(() => {
    let alive = true;
    blobUrl(att.id).then(
      (u) => alive && setUrl(u),
      () => alive && setUrl(null),
    );
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && onClose();
    window.addEventListener('keydown', onKey);
    return () => {
      alive = false;
      window.removeEventListener('keydown', onKey);
    };
  }, [att.id, onClose]);
  const pdf = att.mime === 'application/pdf';
  return (
    <div className="viewer" role="dialog" aria-modal="true" aria-label={att.name}>
      <header>
        <span className="stack" style={{ minWidth: 0 }}>
          <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{att.name}</span>
          <small style={{ opacity: 0.75 }}>{sizeText(att.size)}</small>
        </span>
        <button type="button" className="iconbtn" style={{ color: '#fff' }} aria-label="Закрыть" onClick={onClose}>
          <Icon name="x" size={22} stroke={2} />
        </button>
      </header>
      <div className="vbody">
        {url === undefined && <p style={{ color: '#fff' }}>Открываю…</p>}
        {url === null && <p style={{ color: '#fff' }}>Файл ещё не пришёл с другого устройства.</p>}
        {url && !pdf && <img src={url} alt={att.name} />}
        {url && pdf && (pdfInline ? <iframe title={att.name} src={url} /> : <p style={{ color: '#fff', textAlign: 'center' }}>PDF на телефоне не открывается. Посмотрите его на компьютере.</p>)}
      </div>
    </div>
  );
}

/** Эти операции можно поправить в окне ввода; возврат компании и платежи по долгам — только удалить. */
export const EDITABLE = new Set(['work', 'food', 'personal', 'income', 'advance', 'reimbursement', 'transfer']);

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
          <span style={{ display: 'flex', gap: 4, alignItems: 'center' }}>
            {!!op.att?.length && (
              <span className="faint" title="Есть снимок чека" style={{ display: 'flex' }}>
                <Icon name="clip" size={14} />
              </span>
            )}
            {v.noDoc && <span className="tag warn">без чека</span>}
            {v.chip && <span className={'tag' + (v.chipTagged ? ' grey' : v.chipFamily ? ' own' : '')}>{v.chip}</span>}
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
          {!!op.att?.length && <Thumbs items={op.att} onOpen={act.view} />}
          <div className="wrap">
            {op.kind !== 'transfer' && (
              <button type="button" className="btn ghost" style={{ display: 'flex', alignItems: 'center', gap: 6 }} onClick={() => act.attach(op)}>
                <Icon name="camera" size={18} />
                {op.att?.length ? 'Ещё снимок' : 'Снимок чека'}
              </button>
            )}
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
