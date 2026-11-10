import { useCallback, useEffect, useState } from 'react';
import { api, LOCKED, message, type AiFeature, type AiStatus, type AiUsage, type Snapshot } from '../api';
import { setAi } from '../aiState';
import { NB } from '../format';
import { ErrorText, Icon } from '../ui';

/** «≈ $0,12 (≈ 10 ₽)» или «меньше цента» — оценка по ценам Anthropic; рубли — если есть курс доллара. */
export function costText(v: number, fx?: Record<string, number>): string {
  if (v < 0.005) return 'меньше цента';
  const d = `≈${NB}$${v.toFixed(2).replace('.', ',')}`;
  const rate = fx?.USD;
  return rate ? `${d} (≈${NB}${Math.max(1, Math.round(v * rate))}${NB}₽)` : d;
}

const usageText = (u: AiUsage, fx?: Record<string, number>) => (u.n ? `${u.n} · ${costText(u.usd, fx)}` : 'нет');

const whenText = (at: number) => {
  const d = new Date(at * 1000);
  return `${String(d.getDate()).padStart(2, '0')}.${String(d.getMonth() + 1).padStart(2, '0')} в ${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
};

/** Раздел настроек «ИИ»: ключ API Claude на этом устройстве, распознавание нарядов, счётчик запросов. */
/** Функции ИИ: что делает, какой моделью, как включается. */
const FEATURES: { id: AiFeature; title: string; note: (s: AiStatus) => string; on: string; off: string }[] = [
  { id: 'orders', title: 'Распознавание нарядов', note: (s) => `В поездке → «Наряды» → «Распознать»: ${s.model} читает все листы наряда, время по дням — по правилам переработок`, on: 'Распознавание нарядов включено', off: 'Распознавание нарядов выключено' },
  { id: 'docs', title: 'Билеты, счета гостиниц, приказы', note: (s) => `«Распознать» у снимка в окне ввода и «Из приказа или заявки» в новой поездке — ${s.model}`, on: 'Распознавание документов включено', off: 'Распознавание документов выключено' },
  { id: 'phrase', title: 'Ввод фразой', note: () => 'В окне ввода: «такси 640 до объекта, чек есть» — Claude Haiku 5.5 заполнит форму, сохраняете вы', on: 'Ввод фразой включён', off: 'Ввод фразой выключен' },
  { id: 'chat', title: 'Спросить Claude', note: (s) => `Вопросы о деньгах и поездках — ${s.chat_model}. Claude видит только то, что запросит в «Кармане»`, on: '«Спросить Claude» включено', off: '«Спросить Claude» выключено' },
  { id: 'write', title: 'Claude может записывать', note: () => 'В «Спросить Claude»: операции, дни, наряды, поездки — каждое действие с кнопкой «Вернуть»', on: 'Claude может записывать — каждое действие с «Вернуть»', off: 'Claude только читает' },
  { id: 'review', title: 'Проверка отчёта и сводка месяца', note: (s) => `«Проверить с Claude» в отчёте поездки и сводка месяца в статистике — ${s.chat_model}`, on: 'Проверка и сводка включены', off: 'Проверка и сводка выключены' },
];

export default function AiSection({ snap, flash, locked }: { snap: Snapshot; flash: (text: string, long?: boolean) => void; locked: () => void }) {
  const [st, setStRaw] = useState<AiStatus | null>(null);
  // Состояние — и для других экранов: кнопки «Распознать», «Спросить Claude».
  const setSt = useCallback((s: AiStatus | null) => {
    setStRaw(s);
    setAi(s);
  }, []);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [key, setKey] = useState('');
  const [replace, setReplace] = useState(false);
  const [wsOpen, setWsOpen] = useState(false);
  const [ws, setWs] = useState('');
  const fail = useCallback(
    (e: unknown) => {
      if (message(e) === LOCKED) locked();
      else setError(message(e));
    },
    [locked],
  );
  useEffect(() => {
    api.aiStatus().then((s) => {
      setSt(s);
      setWs(s.workspace);
      if (s.workspace) setWsOpen(true);
    }, fail);
  }, [fail, setSt]);

  if (!st) {
    return (
      <section className="stack">
        <h2 className="h2" style={{ marginBottom: 6 }}>
          ИИ
        </h2>
        {error ? <ErrorText text={error} /> : <p className="note">Загружаю…</p>}
      </section>
    );
  }
  /** Проверка ключа: один короткий запрос к Claude. */
  const check = async (after?: string) => {
    setBusy(true);
    setError(null);
    try {
      const s = await api.aiCheck();
      setSt(s);
      flash(after ? after + ' Ключ работает' : 'Ключ работает');
    } catch (e) {
      fail(e);
      // Ключ сохранён и без проверки — можно поправить пространство и проверить снова.
      api.aiStatus().then(setSt, () => undefined);
    } finally {
      setBusy(false);
    }
  };
  const saveKey = async () => {
    setError(null);
    try {
      const s = await api.aiKeySet(key);
      setSt(s);
      setKey('');
      setReplace(false);
    } catch (e) {
      return fail(e);
    }
    await check('Ключ сохранён.');
  };
  const removeKey = async () => {
    try {
      setSt(await api.aiKeyDelete());
      setError(null);
      flash('Ключ API удалён с этого устройства');
    } catch (e) {
      fail(e);
    }
  };
  const saveWs = async () => {
    try {
      const s = await api.aiWorkspaceSet(ws);
      setSt(s);
      setError(null);
      flash(s.workspace ? 'Пространство сохранено' : 'Пространство не указано');
    } catch (e) {
      fail(e);
    }
  };
  const toggle = async (f: (typeof FEATURES)[number], on: boolean) => {
    try {
      setSt(await api.aiSet(f.id, on));
      flash(on ? f.on : f.off);
    } catch (e) {
      fail(e);
    }
  };
  const keyForm = (
    <div className="stack gap10">
      <ol className="note" style={{ margin: 0, paddingLeft: 20, display: 'grid', gap: 4 }}>
        <li>Войдите в Claude Console (platform.claude.com) и пополните баланс — раздел Billing.</li>
        <li>API keys → Create key. В поле Workspace выберите одно пространство, например Default.</li>
        <li>Скопируйте ключ — Console показывает его один раз — вставьте сюда и нажмите «Подключить»: «Карман» проверит его одним коротким запросом.</li>
      </ol>
      <div className="wrap" style={{ alignItems: 'center' }}>
        <input
          className="field"
          type="password"
          autoComplete="off"
          spellCheck={false}
          aria-label="Ключ API Claude"
          placeholder="sk-ant-…"
          value={key}
          onChange={(e) => setKey(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && key.trim() && saveKey()}
          style={{ flex: '1 1 220px', minWidth: 0 }}
        />
        <button type="button" className="btn work" disabled={busy || !key.trim()} onClick={saveKey}>
          {busy ? 'Проверяю…' : 'Подключить'}
        </button>
        {replace && (
          <button
            type="button"
            className="btn ghost"
            onClick={() => {
              setReplace(false);
              setKey('');
            }}
          >
            Отмена
          </button>
        )}
      </div>
    </div>
  );
  return (
    <section className="stack" id="ai">
      <h2 className="h2" style={{ marginBottom: 6 }}>
        ИИ
      </h2>
      <p className="note" style={{ marginBottom: 8 }}>
        Claude читает наряды, билеты, счета гостиниц и приказы, заполняет операцию по фразе, отвечает на вопросы о деньгах и проверяет отчёт — вы проверяете и сохраняете. Нужен ваш ключ API Claude: запросы оплачиваются с вашего счёта в Claude Console, отдельно от подписки.
      </p>
      {st.key && !replace ? (
        <div className="setrow">
          <span>
            <span>Ключ API</span>
            <span className="note">
              {st.tail} · {st.checked ? 'проверен ' + whenText(st.checked) : 'не проверен'}
            </span>
          </span>
          <span className="wrap" style={{ flex: 'none', justifyContent: 'flex-end' }}>
            <button type="button" className="btn ghost small" disabled={busy} onClick={() => check()}>
              {busy ? 'Проверяю…' : 'Проверить'}
            </button>
            <button type="button" className="btn ghost small" disabled={busy} onClick={() => setReplace(true)}>
              Заменить
            </button>
            <button type="button" className="btn link" disabled={busy} onClick={removeKey}>
              Удалить
            </button>
          </span>
        </div>
      ) : (
        keyForm
      )}
      {FEATURES.map((f) => {
        // Запись — часть «Спросить Claude»: без чата она ни к чему.
        const sub = f.id === 'write';
        const enabled = st.key && (!sub || st.features.chat);
        return (
          <label key={f.id} className="setrow" style={{ opacity: enabled ? 1 : 0.55, paddingLeft: sub ? 18 : undefined }}>
            <span>
              <span>{f.title}</span>
              <span className="note">{f.note(st)}</span>
            </span>
            <input type="checkbox" role="switch" aria-label={f.title} checked={st.features[f.id]} disabled={!enabled} onChange={(e) => toggle(f, e.target.checked)} style={{ flex: 'none', width: 20, height: 20, margin: 0, accentColor: 'var(--own)' }} />
          </label>
        );
      })}
      <div className="setrow">
        <span>
          <span>Запросы к Claude</span>
          <span className="note">
            В этом месяце: {usageText(st.month, snap.fx)}. Всего: {usageText(st.total, snap.fx)}. Сумма — оценка по ценам Anthropic, точная — в Claude Console
          </span>
        </span>
      </div>
      <button type="button" className="collapse" aria-expanded={wsOpen} onClick={() => setWsOpen(!wsOpen)}>
        <span>Ключ не привязан к пространству</span>
        <Icon name={wsOpen ? 'up' : 'down'} size={18} />
      </button>
      {wsOpen && (
        <div className="stack gap6">
          <p className="note">Если ключ создан для всех пространств, Claude API просит ID пространства. Он — в Claude Console на странице Workspaces, столбец ID.</p>
          <div className="wrap" style={{ alignItems: 'center' }}>
            <input className="field" aria-label="ID пространства" spellCheck={false} placeholder="wrkspc_…" value={ws} onChange={(e) => setWs(e.target.value)} style={{ flex: '1 1 220px', minWidth: 0 }} />
            <button type="button" className="btn ghost" disabled={ws.trim() === st.workspace} onClick={saveWs}>
              Применить
            </button>
          </div>
        </div>
      )}
      <p className="note" style={{ marginTop: 8 }}>
        Ключ хранится только на этом устройстве, зашифрованным, — в обмен по Wi-Fi и резервные копии не попадает. В Anthropic уходит только то, что вы отправили сами: листы документа по кнопке «Распознать», фраза, вопрос и данные, которые Claude запросит для ответа, данные поездки или месяца для проверки и сводки.
      </p>
      <ErrorText text={error} />
    </section>
  );
}
