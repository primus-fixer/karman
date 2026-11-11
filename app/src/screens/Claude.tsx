import { useCallback, useEffect, useState } from 'react';
import { api, LOCKED, message, onClaude, type ClaudeStatus, type Snapshot } from '../api';
import { Cut, ErrorText, Icon } from '../ui';

type Run = (p: Promise<Snapshot>, ok?: string) => Promise<Snapshot | null>;

const timeText = (at: number) => {
  const d = new Date(at * 1000);
  return `${String(d.getDate()).padStart(2, '0')}.${String(d.getMonth() + 1).padStart(2, '0')} ${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
};

/** Строка настройки для приложения Claude (claude_desktop_config.json) — если расширение не ставится. */
function manualConfig(s: ClaudeStatus): string {
  return JSON.stringify({ mcpServers: { karman: { command: s.command, args: s.args, env: { KARMAN_DIR: s.dir } } } }, null, 2);
}

/** Раздел настроек «Claude»: коннектор для приложения Claude на этом компьютере. */
export default function ClaudeSection({ mobile, run, flash, locked }: { mobile: boolean; run: Run; flash: (text: string, long?: boolean) => void; locked: () => void }) {
  const [st, setSt] = useState<ClaudeStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [manual, setManual] = useState(false);
  const load = useCallback(() => {
    api.claudeStatus().then(
      (s) => {
        setSt(s);
        setError(null);
      },
      (e) => (message(e) === LOCKED ? locked() : setError(message(e))),
    );
  }, [locked]);
  useEffect(() => {
    if (mobile) return;
    load();
    // Новое действие Claude — в список.
    const un = onClaude(() => load()).catch(() => () => undefined);
    return () => {
      un.then((f) => f());
    };
  }, [load, mobile]);

  if (mobile) {
    return (
      <Cut id="claude" title="Claude">
        <p className="note">Подключение к приложению Claude — в «Кармане» на компьютере: Claude сможет отвечать о деньгах и поездках и записывать операции. Записи придут сюда при обмене по Wi-Fi.</p>
      </Cut>
    );
  }
  if (!st) {
    return (
      <Cut id="claude" title="Claude">
        {error ? <ErrorText text={error} /> : <p className="note">Загружаю…</p>}
      </Cut>
    );
  }
  const set = async (on: boolean, write: boolean) => {
    try {
      setSt(await api.claudeSet(on, write));
      setError(null);
      flash(!on ? 'Подключение для Claude выключено' : write ? 'Claude может записывать — каждое действие с кнопкой «Вернуть»' : 'Подключение для Claude включено: только чтение');
    } catch (e) {
      if (message(e) === LOCKED) locked();
      else setError(message(e));
    }
  };
  const save = async () => {
    setBusy(true);
    try {
      const path = await api.claudeBundle();
      flash('Расширение сохранено: ' + path, true);
    } catch (e) {
      if (message(e) === LOCKED) locked();
      else setError(message(e));
    } finally {
      setBusy(false);
    }
  };
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(manualConfig(st));
      flash('Скопировано: настройка для приложения Claude');
    } catch {
      flash('Не удалось скопировать — выделите текст вручную');
    }
  };
  const undo = async (id: number) => {
    const s = await run(api.claudeUndo(id), 'Отменено');
    if (s) load();
  };
  return (
    <Cut id="claude" title="Claude">
      <p className="note" style={{ marginBottom: 8 }}>
        «Карман» подключается к приложению Claude на этом компьютере. Claude отвечает на вопросы о деньгах и поездках, а если разрешить — записывает операции, рабочие дни и наряды. В интернет «Карман» ничего не открывает; Claude видит только то, что запросит в разговоре.
      </p>
      <label className="setrow">
        <span>
          <span>Подключение для Claude</span>
          <span className="note">Сводка, операции, поездки, переработки, бюджеты и долги. Пока «Карман» заблокирован, Claude ничего не получит</span>
        </span>
        <input type="checkbox" role="switch" aria-label="Подключение для Claude" checked={st.on} onChange={(e) => set(e.target.checked, st.write)} style={{ flex: 'none', width: 20, height: 20, margin: 0, accentColor: 'var(--own)' }} />
      </label>
      <label className="setrow" style={{ opacity: st.on ? 1 : 0.55 }}>
        <span>
          <span>Разрешить запись</span>
          <span className="note">Операции, рабочие дни и наряды, новые поездки, отчёт по переработкам. Каждое действие видно здесь и отменяется кнопкой «Вернуть»</span>
        </span>
        <input type="checkbox" role="switch" aria-label="Разрешить запись для Claude" checked={st.write} disabled={!st.on} onChange={(e) => set(st.on, e.target.checked)} style={{ flex: 'none', width: 20, height: 20, margin: 0, accentColor: 'var(--own)' }} />
      </label>
      <div className="stack gap10" style={{ padding: '12px 0', borderTop: '1px solid var(--line)' }}>
        <span>Как подключить</span>
        <ol className="note" style={{ margin: 0, paddingLeft: 20, display: 'grid', gap: 4 }}>
          <li>Скачайте расширение — файл «Карман для Claude.mcpb» появится в «Загрузках».</li>
          <li>Откройте его двойным щелчком: приложение Claude предложит установить расширение. Или в приложении Claude: Настройки → Extensions → Advanced settings → Install Extension… и выберите файл.</li>
          <li>Спросите Claude, например: «Сколько у меня денег?» или «Запиши такси 640 ₽ в поездку». Если «Карман» закрыт, он откроется сам.</li>
        </ol>
        <button type="button" className="btn ghost" style={{ alignSelf: 'flex-start', display: 'flex', alignItems: 'center', gap: 8 }} disabled={busy} onClick={save}>
          <Icon name="download" size={18} />
          Скачать расширение
        </button>
        <button type="button" className="collapse" aria-expanded={manual} onClick={() => setManual(!manual)}>
          <span>Ручная настройка, если расширение не ставится</span>
          <Icon name={manual ? 'up' : 'down'} size={18} />
        </button>
        {manual && (
          <div className="stack gap6">
            <p className="note">Добавьте в файл настроек приложения Claude (claude_desktop_config.json) и перезапустите его:</p>
            <pre className="codebox">{manualConfig(st)}</pre>
            <button type="button" className="btn ghost small" style={{ alignSelf: 'flex-start' }} onClick={copy}>
              Скопировать
            </button>
          </div>
        )}
      </div>
      {st.actions.length > 0 && (
        <div className="stack" style={{ borderTop: '1px solid var(--line)', paddingTop: 10 }}>
          <span style={{ marginBottom: 4 }}>Последние действия Claude</span>
          {st.actions.slice(0, 20).map((a) => (
            <div key={a.id} className="between" style={{ alignItems: 'center', padding: '6px 0', borderTop: '1px solid var(--line)' }}>
              <span className="stack" style={{ minWidth: 0 }}>
                <span style={{ overflowWrap: 'anywhere', textDecoration: a.undone ? 'line-through' : undefined }}>{a.text}</span>
                <span className="note">{timeText(a.at)}</span>
              </span>
              {a.undone ? (
                <span className="note" style={{ flex: 'none' }}>
                  отменено
                </span>
              ) : (
                <button type="button" className="btn ghost small" style={{ flex: 'none' }} onClick={() => undo(a.id)}>
                  Отменить
                </button>
              )}
            </div>
          ))}
          <p className="note" style={{ marginTop: 6 }}>
            Список — с последнего запуска «Кармана». Сами записи остаются в ленте как обычные.
          </p>
        </div>
      )}
      <ErrorText text={error} />
    </Cut>
  );
}
