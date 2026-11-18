import { useEffect, useRef, useState, type KeyboardEvent } from 'react';
import { api, LOCKED, message, type ChatView, type Snapshot } from '../api';
import { aiOn, useAi } from '../aiState';
import { costText } from './Ai';
import { Icon } from '../ui';

const SUGGEST = ['Сколько у меня денег и что вернуть компании?', 'Что я потратил на такси в этом месяце?', 'Сколько суточных осталось в поездке?', 'Каких чеков не хватает для отчёта?'];

/**
 * «Спросить Claude»: вопросы о деньгах и поездках прямо в «Кармане» (Claude Sonnet 5.5 по ключу API).
 * Claude получает данные только через инструменты «Кармана»; записи — если разрешены, каждая с «Вернуть».
 */
export default function ChatSheet({ snap, isPhone, onClose, locked }: { snap: Snapshot; isPhone: boolean; onClose: () => void; locked: () => void }) {
  const [view, setView] = useState<ChatView | null>(null);
  // Запись разрешают в настройках, пока чат открыт сбоку: состояние ИИ — общее для экранов.
  const ai = useAi();
  const write = ai ? aiOn(ai, 'write') : !!view?.write;
  const [text, setText] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const log = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    api.aiChatView().then(setView, (e) => (message(e) === LOCKED ? locked() : setError(message(e))));
    if (!isPhone) input.current?.focus();
  }, [locked, isPhone]);
  useEffect(() => {
    const onKey = (ev: globalThis.KeyboardEvent) => ev.key === 'Escape' && onClose();
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);
  useEffect(() => {
    log.current?.scrollTo({ top: log.current.scrollHeight });
  }, [view, busy]);

  const send = async (q?: string) => {
    const question = (q ?? text).trim();
    if (!question || busy) return;
    setBusy(true);
    setError(null);
    setText('');
    // Вопрос виден сразу, пока Claude думает.
    setView((v) => (v ? { ...v, items: [...v.items, { role: 'user', text: question, actions: [], error: false, confirm: [] }] } : v));
    try {
      setView(await api.aiChatSend(question));
    } catch (e) {
      if (message(e) === LOCKED) return locked();
      setError(message(e));
      api.aiChatView().then(setView, () => undefined);
    } finally {
      setBusy(false);
    }
  };
  const decide = async (id: number, yes: boolean) => {
    setBusy(true);
    setError(null);
    try {
      setView(await api.aiChatConfirm(id, yes));
    } catch (e) {
      if (message(e) === LOCKED) return locked();
      setError(message(e));
    } finally {
      setBusy(false);
    }
  };
  const reset = async () => {
    try {
      setView(await api.aiChatReset());
      setError(null);
      input.current?.focus();
    } catch (e) {
      setError(message(e));
    }
  };
  const keys = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      send();
    }
  };
  const items = view?.items ?? [];
  return (
    <section className="chatsheet" role="dialog" aria-label="Спросить Claude">
      <div className="between" style={{ alignItems: 'center', gap: 6 }}>
        <h2 className="sec" style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <Icon name="spark" size={20} />
          Спросить Claude
        </h2>
        <span style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
          {items.length > 0 && (
            <button type="button" className="link-btn warn-text" disabled={busy} onClick={reset}>
              Новый разговор
            </button>
          )}
          <button type="button" className="iconbtn" aria-label="Закрыть" onClick={onClose}>
            <Icon name="x" size={20} stroke={2} />
          </button>
        </span>
      </div>
      <div className="chatlog" ref={log} aria-live="polite">
        {items.length === 0 && !busy && (
          <div className="stack gap10" style={{ padding: '6px 0' }}>
            <p className="small muted">Спросите о деньгах, поездках, переработках, бюджете или долгах. Claude видит только то, что запросит в «Кармане».</p>
            <div className="chips">
              {SUGGEST.map((q) => (
                <button key={q} type="button" className="chip" onClick={() => send(q)}>
                  {q}
                </button>
              ))}
            </div>
          </div>
        )}
        {items.map((m, i) => (
          <div key={i} className={'chatmsg ' + m.role + (m.error ? ' error' : '')}>
            <span>{m.text}</span>
            {m.actions.length > 0 && (
              <span className="chatacts">
                {m.actions.map((a, j) => (
                  <span key={j}>✓ {a}</span>
                ))}
                <span className="small muted">Отменить — кнопкой «Вернуть» в сообщении внизу или в Настройки → Claude</span>
              </span>
            )}
            {(m.confirm ?? []).map((c) => (
              <span key={c.id} className="chatconfirm" role="group" aria-label="Claude предлагает удалить">
                <span>{c.text}</span>
                {c.state === 'wait' ? (
                  <span className="sec-actions">
                    <button type="button" className="btn warn" disabled={busy} onClick={() => decide(c.id, true)}>
                      {c.button}
                    </button>
                    <button type="button" className="btn line" disabled={busy} onClick={() => decide(c.id, false)}>
                      Оставить
                    </button>
                  </span>
                ) : (
                  <span className="small muted">{c.state === 'done' ? '✓ Сделано — отменить можно кнопкой «Вернуть»' : c.state === 'kept' ? 'Оставлено как есть' : 'Не получилось'}</span>
                )}
              </span>
            ))}
          </div>
        ))}
        {busy && <div className="chatmsg assistant muted3">Claude думает…</div>}
      </div>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      <div className="chatin">
        <textarea ref={input} className="input" rows={2} aria-label="Вопрос Claude" placeholder="Например: сколько я потратил на такси в октябре?" value={text} maxLength={2000} onChange={(e) => setText(e.target.value)} onKeyDown={keys} />
        <button type="button" className="btn work" disabled={busy || !text.trim()} onClick={() => send()}>
          Спросить
        </button>
      </div>
      <p className="small muted" style={{ fontSize: 12, lineHeight: '16px' }}>
        {write ? 'Claude может записывать — каждое действие с «Вернуть».' : 'Claude только читает; запись включается в Настройки → ИИ.'}
        {view && view.usd > 0 ? ' Разговор: ' + costText(view.usd, snap.fx) + '.' : ''}
      </p>
    </section>
  );
}
