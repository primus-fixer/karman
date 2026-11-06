import { useCallback, useEffect, useState } from 'react';
import {
  api,
  inApp,
  message,
  scanQr,
  type CalcCheck,
  type Check,
  type DbCheck,
  type DeviceInfo,
  type OpView,
  type PairingView,
  type ReceiptView,
  type SyncResult,
  type SyncStatus,
} from './api';

function Mark({ ok }: { ok: boolean }) {
  return (
    <svg className={ok ? 'ok' : 'bad'} width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" aria-label={ok ? 'Да' : 'Нет'}>
      <path d={ok ? 'M20 6 9 17l-5-5' : 'M18 6 6 18M6 6l12 12'} />
    </svg>
  );
}

function Checks({ items }: { items: Check[] }) {
  return (
    <ul className="checks">
      {items.map((c, i) => (
        <li key={i}>
          <Mark ok={c.ok} />
          <span>{c.text}</span>
        </li>
      ))}
    </ul>
  );
}

function ErrorText({ text }: { text: string | null }) {
  return text ? <p className="error" role="alert">{text}</p> : null;
}

/** Запуск действия с индикатором и ошибкой. */
function useAction<T>() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = useCallback(async (fn: () => Promise<T>): Promise<T | undefined> => {
    setBusy(true);
    setError(null);
    try {
      return await fn();
    } catch (e) {
      setError(message(e));
      return undefined;
    } finally {
      setBusy(false);
    }
  }, []);
  return { busy, error, run, setError };
}

function DeviceCard({ info }: { info: DeviceInfo | null }) {
  if (!info) return null;
  const rows: [string, string][] = [
    ['Устройство', info.name],
    ['Система', `${info.os}, ${info.arch}`],
    ['Номер устройства', info.id],
    ['База', info.encrypted ? `зашифрована, SQLCipher ${info.cipher}` : 'НЕ зашифрована'],
    ['Событий в журнале', String(info.events)],
    ['Операций', String(info.ops)],
  ];
  return (
    <section className="card">
      <h2>Это устройство</h2>
      <div className="rows">
        {rows.map(([k, v]) => (
          <div className="row" key={k}>
            <span>{k}</span>
            <span>{v}</span>
          </div>
        ))}
      </div>
      <p className="note">Папка данных: {info.data_dir}</p>
    </section>
  );
}

function DbCard({ onDone }: { onDone: () => void }) {
  const [res, setRes] = useState<DbCheck | null>(null);
  const a = useAction<DbCheck>();
  return (
    <section className="card">
      <h2>Зашифрованная база</h2>
      <p className="note">Создаёт отдельную проверочную базу, пишет 2 000 строк, открывает её заново и пробует чужой ключ. Потом удаляет.</p>
      <div className="actions">
        <button className="btn" disabled={a.busy} onClick={async () => { const r = await a.run(api.dbCheck); if (r) { setRes(r); onDone(); } }}>
          {a.busy ? 'Проверяю…' : 'Проверить базу'}
        </button>
      </div>
      <ErrorText text={a.error} />
      {res && <Checks items={res.checks} />}
    </section>
  );
}

function CalcCard() {
  const [res, setRes] = useState<CalcCheck | null>(null);
  const a = useAction<CalcCheck>();
  return (
    <section className="card">
      <h2>Расчёт поездок</h2>
      <p className="note">Поездки из макета. Суточные по ставке, траты без документов — сначала из суточных, остальное — долг перед компанией.</p>
      <div className="actions">
        <button className="btn work" disabled={a.busy} onClick={async () => { const r = await a.run(api.calcCheck); if (r) setRes(r); }}>
          Посчитать
        </button>
      </div>
      <ErrorText text={a.error} />
      {res && (
        <>
          <p className="note">Ставка суточных: {res.rate}</p>
          <Checks items={res.checks} />
          <div>
            {res.trips.map((t) => (
              <div className="trip" key={t.name}>
                <b>{t.name}</b>
                <small>{t.status}. Израсходовано по отчёту {t.fact}</small>
                <span>{t.headline}</span>
                <small>{t.per_diem}</small>
                {t.over_note && <small>Сверх бюджета: {t.over_note}</small>}
              </div>
            ))}
          </div>
        </>
      )}
    </section>
  );
}

function SyncCard({ mobile, onChange }: { mobile: boolean; onChange: () => void }) {
  const [pairing, setPairing] = useState<PairingView | null>(null);
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [uri, setUri] = useState('');
  const [result, setResult] = useState<SyncResult | null>(null);
  const [ops, setOps] = useState<OpView[]>([]);
  const [amount, setAmount] = useState('350');
  const a = useAction<unknown>();

  const refreshOps = useCallback(async () => setOps(await api.opsList()), []);
  useEffect(() => {
    refreshOps().catch(() => undefined);
  }, [refreshOps]);

  // Пока показан QR, раз в 2 секунды спрашиваем, кто подключался.
  useEffect(() => {
    if (!pairing) return;
    let alive = true;
    const tick = async () => {
      try {
        const s = await api.syncStatus();
        if (!alive) return;
        setStatus((prev) => {
          if (prev && s.log.length > prev.log.length) {
            refreshOps().catch(() => undefined);
            onChange();
          }
          return s;
        });
      } catch {
        /* окно закрывается */
      }
    };
    tick();
    const id = window.setInterval(tick, 2000);
    return () => {
      alive = false;
      window.clearInterval(id);
    };
  }, [pairing, refreshOps, onChange]);

  const connect = async (text: string) => {
    const r = (await a.run(() => api.syncConnect(text))) as SyncResult | undefined;
    if (r) {
      setResult(r);
      setOps(r.ops);
      onChange();
    }
  };

  return (
    <section className="card">
      <h2>Связь устройств по Wi-Fi</h2>
      <p className="note">
        На компьютере нажмите «Показать QR-код», на телефоне — «Сканировать». Устройства должны быть в одной сети Wi-Fi или телефон
        раздаёт точку доступа. Данные идут зашифрованными.
      </p>
      <div className="actions">
        {!pairing ? (
          <button className="btn own" disabled={a.busy} onClick={async () => { const p = (await a.run(api.syncStart)) as PairingView | undefined; if (p) setPairing(p); }}>
            Показать QR-код
          </button>
        ) : (
          <button className="btn ghost" onClick={async () => { await a.run(api.syncStop); setPairing(null); setStatus(null); }}>
            Перестать ждать
          </button>
        )}
        {mobile && (
          <button className="btn" disabled={a.busy} onClick={async () => { const text = (await a.run(scanQr)) as string | undefined; if (text) await connect(text); }}>
            Сканировать QR-код
          </button>
        )}
      </div>
      {pairing && (
        <>
          <div className="qr" dangerouslySetInnerHTML={{ __html: pairing.qr_svg }} aria-label="QR-код для связи" role="img" />
          <p className="note">
            Жду подключения: {pairing.hosts.map((h) => `${h}:${pairing.port}`).join(', ')}. Если камеры нет, передайте другому устройству
            ссылку: <span style={{ overflowWrap: 'anywhere' }}>{pairing.uri}</span>
          </p>
          {status && status.log.length > 0 && <Checks items={status.log} />}
        </>
      )}
      <textarea className="field" rows={2} placeholder="Или вставьте ссылку karman://pair… с другого устройства" value={uri} onChange={(e) => setUri(e.target.value)} />
      <div className="actions">
        <button className="btn ghost" disabled={a.busy || !uri.trim()} onClick={() => connect(uri.trim())}>
          {a.busy ? 'Связываюсь…' : 'Подключиться по ссылке'}
        </button>
      </div>
      <ErrorText text={a.error} />
      {result && <Checks items={[{ ok: true, text: `Обмен с «${result.peer_name}»: отправлено ${result.sent}, получено ${result.received}` }]} />}

      <h2 style={{ marginTop: 6 }}>Тестовые операции</h2>
      <p className="note">Добавьте операцию на одном устройстве и обменяйтесь — она появится на втором.</p>
      <div className="actions">
        <input className="field num" inputMode="decimal" value={amount} onChange={(e) => setAmount(e.target.value)} aria-label="Сумма, рублей" style={{ width: 120 }} />
        <button className="btn ghost" disabled={a.busy} onClick={async () => { const r = (await a.run(() => api.opAddTest(amount))) as OpView[] | undefined; if (r) { setOps(r); onChange(); } }}>
          Добавить операцию
        </button>
      </div>
      <div>
        {ops.length === 0 && <p className="note">Операций пока нет.</p>}
        {ops.map((o) => (
          <div className="op" key={o.id}>
            <span>
              {o.title}
              <small>{o.date}</small>
            </span>
            <span className="num">{o.amount}</span>
            <button className="link" onClick={async () => { const r = (await a.run(() => api.opDelete(o.id))) as OpView[] | undefined; if (r) { setOps(r); onChange(); } }}>
              Удалить
            </button>
          </div>
        ))}
      </div>
    </section>
  );
}

function ReceiptCard({ mobile }: { mobile: boolean }) {
  const [text, setText] = useState('t=20261031T1532&s=600.00&fn=7380440700076549&i=41218&fp=2489764517&n=1');
  const [res, setRes] = useState<ReceiptView | null>(null);
  const a = useAction<ReceiptView>();
  const parse = async (t: string) => {
    const r = await a.run(() => api.receiptParse(t));
    if (r) setRes(r);
  };
  return (
    <section className="card">
      <h2>Чек по QR-коду</h2>
      <p className="note">QR-код на кассовом чеке даёт дату, сумму и данные для проверки в ФНС.</p>
      <div className="actions">
        {mobile && (
          <button className="btn" disabled={a.busy} onClick={async () => { try { const t = await scanQr(); setText(t); await parse(t); } catch (e) { a.setError(message(e)); } }}>
            Сканировать чек
          </button>
        )}
      </div>
      <textarea className="field" rows={2} value={text} onChange={(e) => setText(e.target.value)} aria-label="Текст QR-кода чека" />
      <div className="actions">
        <button className="btn ghost" disabled={a.busy || !text.trim()} onClick={() => parse(text)}>
          Разобрать
        </button>
      </div>
      <ErrorText text={a.error} />
      {res && (
        <div className="rows">
          {(
            [
              ['Дата и время', `${res.date}, ${res.time}`],
              ['Сумма', res.sum],
              ['Тип', res.kind],
              ['ФН', res.fn_number],
              ['ФД', res.fd],
              ['ФП', res.fp],
            ] as [string, string][]
          ).map(([k, v]) => (
            <div className="row" key={k}>
              <span>{k}</span>
              <span className={k === 'Сумма' ? 'num' : undefined}>{v}</span>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}

export default function App() {
  const [info, setInfo] = useState<DeviceInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const refresh = useCallback(() => {
    api.deviceInfo().then(setInfo, (e) => setError(message(e)));
  }, []);
  useEffect(() => {
    if (inApp) refresh();
  }, [refresh]);
  const mobile = info ? info.os === 'android' || info.os === 'ios' : false;

  return (
    <main className="page">
      <header>
        <p className="brand">Карман</p>
        <p className="lead">Этап 0: проверяем на ваших устройствах базу, расчёты и связь по Wi-Fi.</p>
      </header>
      {!inApp && <p className="error">Эта страница работает внутри приложения «Карман». В обычном браузере команды недоступны.</p>}
      <ErrorText text={error} />
      <div className="grid">
        <DeviceCard info={info} />
        <DbCard onDone={refresh} />
        <CalcCard />
        <SyncCard mobile={mobile} onChange={refresh} />
        <ReceiptCard mobile={mobile} />
        <section className="card">
          <h2>Облако и свой сервер</h2>
          <p className="note">
            Яндекс Диск и Google Drive подключим после регистрации приложений у Яндекса и Google — это следующий шаг. Сервер для VPS или NAS
            появится вместе с приглашениями для семьи и коллег.
          </p>
        </section>
      </div>
    </main>
  );
}
