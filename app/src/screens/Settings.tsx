import { useEffect, useRef, useState } from 'react';
import { api, message, scanQr, type Account, type AccountView, type Check, type DeviceInfo, type LockStatus, type PairingView, type Snapshot, type SyncResult } from '../api';
import type { PinMode } from '../Lock';
import { SIGN, amountText, parseAmount, rub } from '../format';
import { SCHEMES, type ThemeMode } from '../theme';
import { Checks, ErrorText, Icon, Seg } from '../ui';

export type ThemeState = { scheme: string; mode: ThemeMode };

export type Security = {
  lock: LockStatus | null;
  onPin: (mode: PinMode) => void;
  onBio: (on: boolean) => void;
  onLockNow: () => void;
};

function SecuritySection({ sec, os }: { sec: Security; os: string }) {
  const lock = sec.lock;
  if (!lock) return null;
  const win = os === 'windows';
  const droid = os === 'android';
  const bioTitle = win ? 'Вход через Windows Hello' : lock.bio_kind === 'face' ? 'Вход по лицу' : 'Вход по отпечатку';
  let bioNote: string;
  if (!lock.pin) bioNote = 'Сначала задайте PIN-код — он останется запасным';
  else if (!lock.bio_ready)
    bioNote = win ? 'Windows Hello не настроен: Параметры → Учётные записи → Варианты входа' : droid ? 'На телефоне не добавлен отпечаток пальца' : 'На этом устройстве недоступно';
  else bioNote = win ? 'Отпечаток, лицо или PIN Windows — что настроено в Windows Hello. PIN-код «Кармана» остаётся запасным' : 'PIN-код остаётся запасным';
  const keyText = lock.key === 'dpapi' ? 'зашифрован учётной записью Windows' : lock.key === 'android-keystore' ? 'в защищённом хранилище телефона' : 'в файле рядом с базой';
  const recover = win ? 'через Windows Hello' : droid ? 'блокировкой экрана телефона' : 'средствами системы';
  return (
    <section className="stack">
      <h2 className="h2" style={{ marginBottom: 6 }}>
        Защита
      </h2>
      <div className="setrow">
        <span>
          <span>Вход по PIN-коду</span>
          <span className="note">{lock.pin ? 'Спрашивается при запуске и после 5 минут в фоне' : 'Выключен: «Карман» открывается без кода'}</span>
        </span>
        {!lock.pin && (
          <button type="button" className="btn own" style={{ flex: 'none' }} onClick={() => sec.onPin('create')}>
            Задать PIN
          </button>
        )}
      </div>
      {lock.pin && (
        <div className="wrap" style={{ padding: '2px 0 12px' }}>
          <button type="button" className="btn ghost" onClick={() => sec.onPin('change')}>
            Сменить PIN
          </button>
          <button type="button" className="btn ghost" onClick={() => sec.onPin('off')}>
            Выключить
          </button>
          <button type="button" className="btn ghost" style={{ display: 'flex', alignItems: 'center', gap: 8 }} onClick={sec.onLockNow}>
            <Icon name="lock" size={18} />
            Заблокировать сейчас
          </button>
        </div>
      )}
      <label className="setrow" style={{ opacity: lock.pin && lock.bio_ready ? 1 : 0.65, cursor: lock.pin && lock.bio_ready ? 'pointer' : 'default' }}>
        <span>
          <span>{bioTitle}</span>
          <span className="note">{bioNote}</span>
        </span>
        <input
          type="checkbox"
          role="switch"
          aria-label={bioTitle}
          checked={lock.bio}
          disabled={!lock.pin || !lock.bio_ready}
          onChange={(e) => sec.onBio(e.target.checked)}
          style={{ flex: 'none', width: 20, height: 20, margin: 0, accentColor: 'var(--own)' }}
        />
      </label>
      <div className="rule">
        <span>Ключ базы</span>
        <span>{keyText}</span>
      </div>
      {lock.pin && (
        <p className="note" style={{ marginTop: 6 }}>
          Забыли PIN-код — на экране входа нажмите «Забыли PIN?» и подтвердите себя {recover}. Данные останутся.
        </p>
      )}
    </section>
  );
}

const RULES: [string, string][] = [
  ['Аванс', 'равен сумме заявки'],
  ['Расход без документа', 'из суточных, сверх них — долг'],
  ['Остаток аванса', 'возвращается компании'],
  ['Перерасход по документам', 'возмещает компания'],
  ['Материалы и инструменты', 'статья 03.01.04.05'],
  ['Статьи отчёта', '03.01.04.01–05'],
];

const KINDS: { id: string; label: string; note: string }[] = [
  { id: 'work', label: 'Рабочий', note: 'Сюда приходит аванс, с него возврат компании' },
  { id: 'personal', label: 'Личный', note: 'Сюда приходит возмещение и зарплата' },
  { id: 'cash', label: 'Наличные', note: 'Деньги в кошельке' },
];

const CURRENCIES = ['RUB', 'KZT', 'USD', 'EUR'];

function parseSigned(s: string): number | null {
  const t = s.trim();
  const neg = /^[-−]/.test(t);
  const k = parseAmount(t.replace(/^[-−]\s*/, ''));
  return k == null ? null : neg ? -k : k;
}

function AccountEditor({
  snap,
  view,
  onSave,
  onCancel,
}: {
  snap: Snapshot;
  view: AccountView | null;
  onSave: (a: Account) => Promise<boolean>;
  onCancel: () => void;
}) {
  const base: Account = view
    ? view.account
    : { id: '', name: '', short: '', currency: 'RUB', opening: 0, kind: 'personal', order: Math.max(0, ...snap.accounts.map((a) => a.account.order)) + 1, archived: false };
  const moved = view ? view.balance - view.account.opening : 0;
  const used = !!view && snap.ops.some((o) => o.account === view.account.id);
  const [name, setName] = useState(base.name);
  const [short, setShort] = useState(base.short);
  const [currency, setCurrency] = useState(base.currency);
  const [kind, setKind] = useState(base.kind);
  const [archived, setArchived] = useState(base.archived);
  const [bal, setBal] = useState(view ? (view.balance < 0 ? '-' : '') + amountText(view.balance) : '');
  const [error, setError] = useState<string | null>(null);
  const save = async () => {
    if (!name.trim()) return setError('Укажите название счёта');
    const b = bal.trim() ? parseSigned(bal) : 0;
    if (b == null) return setError('Не получается прочитать остаток');
    const ok = await onSave({ ...base, name: name.trim(), short: short.trim(), currency, kind, archived, opening: b - moved });
    if (!ok) setError('Не удалось сохранить');
  };
  return (
    <div className="stack gap12" style={{ padding: '12px 0 16px', borderTop: '1px solid var(--line)' }}>
      <div className="form-grid">
        <label className="label">
          <span>Название</span>
          <input className="field" value={name} onChange={(e) => setName(e.target.value)} placeholder="Например, Карта Сбера" />
        </label>
        <label className="label">
          <span>Коротко, для кнопок ввода</span>
          <input className="field" value={short} onChange={(e) => setShort(e.target.value)} placeholder="Сбер" maxLength={14} />
        </label>
        <label className="label">
          <span>Остаток сейчас, {SIGN[currency] || currency}</span>
          <input className="field num" inputMode="decimal" value={bal} onChange={(e) => setBal(e.target.value)} placeholder="0" />
        </label>
        <label className="label">
          <span>Валюта</span>
          <select className="field" value={currency} disabled={used} onChange={(e) => setCurrency(e.target.value)}>
            {CURRENCIES.concat(CURRENCIES.includes(currency) ? [] : [currency]).map((c) => (
              <option key={c} value={c}>
                {c} {SIGN[c] || ''}
              </option>
            ))}
          </select>
        </label>
      </div>
      {used && <p className="note">Валюту нельзя поменять: по счёту уже есть операции.</p>}
      <Seg label="Тип счёта" value={kind} onPick={setKind} items={KINDS.map((k) => ({ id: k.id, label: k.label }))} />
      <p className="note" style={{ marginTop: -6 }}>
        {KINDS.find((k) => k.id === kind)?.note}
      </p>
      {view && (
        <label style={{ display: 'flex', alignItems: 'center', gap: 10, minHeight: 44 }}>
          <input type="checkbox" checked={archived} onChange={(e) => setArchived(e.target.checked)} style={{ width: 20, height: 20, margin: 0, accentColor: 'var(--own)' }} />
          <span>Скрыть счёт: не показывать в обзоре и при вводе</span>
        </label>
      )}
      <ErrorText text={error} />
      <div className="wrap">
        <button type="button" className="btn own" onClick={save}>
          Сохранить
        </button>
        <button type="button" className="btn ghost" onClick={onCancel}>
          Отмена
        </button>
      </div>
    </div>
  );
}

function SyncWifi({ mobile, onSynced }: { mobile: boolean; onSynced: () => void }) {
  const [pairing, setPairing] = useState<PairingView | null>(null);
  const [log, setLog] = useState<Check[]>([]);
  const [uri, setUri] = useState('');
  const [result, setResult] = useState<SyncResult | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const seen = useRef(0);

  // Пока показан QR-код, раз в 2 секунды спрашиваем, кто подключался.
  useEffect(() => {
    if (!pairing) return;
    let alive = true;
    const tick = async () => {
      try {
        const s = await api.syncStatus();
        if (!alive) return;
        setLog(s.log);
        if (s.log.length > seen.current) {
          seen.current = s.log.length;
          onSynced();
        }
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
  }, [pairing, onSynced]);

  const act = async <T,>(fn: () => Promise<T>): Promise<T | undefined> => {
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
  };
  const connect = async (text: string) => {
    const r = await act(() => api.syncConnect(text));
    if (r) {
      setResult(r);
      setUri('');
      onSynced();
    }
  };

  return (
    <div className="stack gap10" style={{ paddingTop: 8 }}>
      <p className="note">
        На компьютере нажмите «Показать QR-код», на телефоне — «Сканировать QR-код». Устройства должны быть в одной сети Wi-Fi, или телефон раздаёт точку доступа. Данные идут зашифрованными.
      </p>
      <div className="wrap">
        {!pairing ? (
          <button
            type="button"
            className="btn own"
            disabled={busy}
            onClick={async () => {
              const p = await act(api.syncStart);
              if (p) {
                seen.current = 0;
                setLog([]);
                setPairing(p);
              }
            }}
          >
            Показать QR-код
          </button>
        ) : (
          <button
            type="button"
            className="btn ghost"
            onClick={async () => {
              await act(api.syncStop);
              setPairing(null);
            }}
          >
            Перестать ждать
          </button>
        )}
        {mobile && (
          <button
            type="button"
            className="btn"
            disabled={busy}
            onClick={async () => {
              const text = await act(scanQr);
              if (text) await connect(text);
            }}
          >
            Сканировать QR-код
          </button>
        )}
      </div>
      {pairing && (
        <>
          <div className="qr" dangerouslySetInnerHTML={{ __html: pairing.qr_svg }} aria-label="QR-код для связи" role="img" />
          <p className="note">
            Жду подключения: {pairing.hosts.map((h) => `${h}:${pairing.port}`).join(', ')}. Если камеры нет, передайте другому устройству ссылку:{' '}
            <span style={{ overflowWrap: 'anywhere' }}>{pairing.uri}</span>
          </p>
          {log.length > 0 && <Checks items={log} />}
        </>
      )}
      <textarea className="field" rows={2} style={{ padding: '10px 12px' }} placeholder="Или вставьте ссылку karman://pair… с другого устройства" value={uri} onChange={(e) => setUri(e.target.value)} />
      <div className="wrap">
        <button type="button" className="btn ghost" disabled={busy || !uri.trim()} onClick={() => connect(uri.trim())}>
          {busy ? 'Связываюсь…' : 'Подключиться по ссылке'}
        </button>
      </div>
      <ErrorText text={error} />
      {result && (
        <Checks
          items={[
            {
              ok: true,
              text:
                `Обмен с «${result.peer_name}»: отправлено ${result.sent}, получено ${result.received}` +
                (result.files_sent + result.files_received ? `; снимков чеков: отправлено ${result.files_sent}, получено ${result.files_received}` : ''),
            },
          ]}
        />
      )}
    </div>
  );
}

function CheckButton({ label, run }: { label: string; run: () => Promise<Check[]> }) {
  const [res, setRes] = useState<Check[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  return (
    <div className="stack gap10">
      <button
        type="button"
        className="btn ghost"
        style={{ alignSelf: 'flex-start' }}
        disabled={busy}
        onClick={async () => {
          setBusy(true);
          setError(null);
          try {
            setRes(await run());
          } catch (e) {
            setError(message(e));
          } finally {
            setBusy(false);
          }
        }}
      >
        {busy ? 'Проверяю…' : label}
      </button>
      <ErrorText text={error} />
      {res && <Checks items={res} />}
    </div>
  );
}

export default function Settings({
  snap,
  info,
  mobile,
  run,
  refresh,
  theme,
  setTheme,
  dark,
  security,
}: {
  snap: Snapshot;
  info: DeviceInfo | null;
  mobile: boolean;
  run: (p: Promise<Snapshot>, ok?: string) => Promise<Snapshot | null>;
  refresh: () => void;
  theme: ThemeState;
  setTheme: (t: ThemeState) => void;
  dark: boolean;
  security: Security;
}) {
  const [rateText, setRateText] = useState(amountText(snap.rate));
  useEffect(() => setRateText(amountText(snap.rate)), [snap.rate]);
  const [editAcc, setEditAcc] = useState<string | null>(null);
  const [showChecks, setShowChecks] = useState(false);
  const [rateError, setRateError] = useState<string | null>(null);

  const commitRate = async () => {
    const k = parseAmount(rateText);
    if (k == null || k > 10_000_000) return setRateError('Введите сумму в рублях, например 700');
    setRateError(null);
    if (k !== snap.rate) await run(api.settingSet('rate', String(k)), 'Ставка суточных: ' + rub(k) + ' в день');
  };

  const fxCurrencies = Array.from(new Set(['KZT', 'USD', 'EUR', ...snap.accounts.map((a) => a.account.currency)])).filter((c) => c !== 'RUB');

  return (
    <div className="stack gap26">
      <h1 className="h1">Настройки</h1>

      <section className="stack">
        <h2 className="h2" style={{ marginBottom: 6 }}>
          Командировки
        </h2>
        <label className="setrow">
          <span>
            <span>Суточные в день, ₽</span>
            <span className="note">Для поездок, по которым отчёт ещё не сдан. В поездке можно задать свою ставку</span>
          </span>
          <input
            className="field num"
            inputMode="decimal"
            aria-label="Суточные в день, рублей"
            value={rateText}
            onChange={(e) => setRateText(e.target.value)}
            onBlur={commitRate}
            onKeyDown={(e) => e.key === 'Enter' && (e.target as HTMLInputElement).blur()}
            style={{ flex: 'none', width: 96, height: 40, textAlign: 'right', fontSize: 16 }}
          />
        </label>
        <ErrorText text={rateError} />
        {RULES.map(([k, v]) => (
          <div className="rule" key={k}>
            <span>{k}</span>
            <span>{v}</span>
          </div>
        ))}
      </section>

      <section className="stack">
        <h2 className="h2" style={{ marginBottom: 6 }}>
          Счета
        </h2>
        <p className="note" style={{ marginBottom: 8 }}>
          Укажите, сколько сейчас на каждом счёте, — дальше остатки считаются по записанным операциям.
        </p>
        {snap.accounts.map((a) =>
          editAcc === a.account.id ? (
            <AccountEditor
              key={a.account.id}
              snap={snap}
              view={a}
              onCancel={() => setEditAcc(null)}
              onSave={async (acc) => {
                const ok = !!(await run(api.accountSave(acc), 'Счёт сохранён'));
                if (ok) setEditAcc(null);
                return ok;
              }}
            />
          ) : (
            <button key={a.account.id} type="button" className="listbtn" onClick={() => setEditAcc(a.account.id)} style={{ minHeight: 56, opacity: a.account.archived ? 0.6 : 1 }}>
              <span className="stack" style={{ minWidth: 0 }}>
                <span>{a.account.name}</span>
                <span className="note">
                  {(KINDS.find((k) => k.id === a.account.kind)?.label ?? 'Счёт') + (a.account.archived ? ', скрыт' : '')}
                </span>
              </span>
              <span className="num" style={{ flex: 'none', textAlign: 'right' }}>
                {rub(a.balance, a.account.currency)}
                {a.account.currency !== 'RUB' && <span className="note faint" style={{ display: 'block' }}>≈ {rub(a.balance_rub)}</span>}
              </span>
            </button>
          ),
        )}
        {editAcc === 'new' ? (
          <AccountEditor
            snap={snap}
            view={null}
            onCancel={() => setEditAcc(null)}
            onSave={async (acc) => {
              const ok = !!(await run(api.accountSave(acc), 'Счёт добавлен'));
              if (ok) setEditAcc(null);
              return ok;
            }}
          />
        ) : (
          <button type="button" className="btn ghost" style={{ marginTop: 10, alignSelf: 'flex-start' }} onClick={() => setEditAcc('new')}>
            Добавить счёт
          </button>
        )}
      </section>

      <section className="stack">
        <h2 className="h2" style={{ marginBottom: 6 }}>
          Курсы валют
        </h2>
        <p className="note" style={{ marginBottom: 8 }}>
          Сколько рублей за единицу валюты. Пока курсы задаются вручную.
        </p>
        {fxCurrencies.map((c) => (
          <FxRow key={c} cur={c} value={snap.fx[c]} save={(v) => run(api.settingSet('fx.' + c, v), `Курс ${c} сохранён`)} />
        ))}
      </section>

      <section className="stack">
        <h2 className="h2" style={{ marginBottom: 6 }}>
          Синхронизация
        </h2>
        <div className="setrow" style={{ minHeight: 58 }}>
          <span style={{ flexDirection: 'row', alignItems: 'center', gap: 12 }}>
            <Icon name="wifi" size={22} />
            <span className="stack">
              <span>Wi-Fi напрямую</span>
              <span className="note">Компьютер и телефон в одной сети</span>
            </span>
          </span>
        </div>
        <SyncWifi mobile={mobile} onSynced={refresh} />
        {[
          ['cloud', 'Личное облако', 'Яндекс Диск и Google Drive — следующий шаг'],
          ['server', 'Личный сервер', 'VPS или NAS — вместе с совместным доступом'],
        ].map(([icon, name, note]) => (
          <div key={icon} className="setrow" style={{ minHeight: 58, marginTop: icon === 'cloud' ? 14 : 0 }}>
            <span style={{ flexDirection: 'row', alignItems: 'center', gap: 12 }}>
              <Icon name={icon} size={22} />
              <span className="stack">
                <span>{name}</span>
                <span className="note">{note}</span>
              </span>
            </span>
            <span className="note faint">скоро</span>
          </div>
        ))}
      </section>

      <section className="stack gap10">
        <h2 className="h2">Оформление</h2>
        <Seg
          label="Тема"
          value={theme.mode}
          onPick={(mode) => setTheme({ ...theme, mode })}
          items={[
            { id: 'light', label: 'Светлая' },
            { id: 'dark', label: 'Тёмная' },
            { id: 'system', label: 'Как в системе' },
          ]}
        />
        <div className="schemes" role="group" aria-label="Цветовая схема">
          {Object.keys(SCHEMES).map((id) => {
            const v = SCHEMES[id][dark ? 'dark' : 'light'];
            const on = id === theme.scheme;
            return (
              <button key={id} type="button" aria-pressed={on} className={'scheme' + (on ? ' on' : '')} style={{ background: v.bg, color: v.ink }} onClick={() => setTheme({ ...theme, scheme: id })}>
                <span style={{ display: 'flex', gap: 4 }}>
                  <span style={{ width: 18, height: 18, background: v.own }} />
                  <span style={{ width: 18, height: 18, background: v.work }} />
                </span>
                <span>{SCHEMES[id].name}</span>
              </button>
            );
          })}
        </div>
      </section>

      <SecuritySection sec={security} os={info?.os ?? ''} />

      <section className="stack">
        <h2 className="h2" style={{ marginBottom: 6 }}>
          Это устройство
        </h2>
        {info ? (
          <>
            <div className="row">
              <span>Устройство</span>
              <span>{info.name}</span>
            </div>
            <div className="row">
              <span>Система</span>
              <span>
                {info.os}, {info.arch}
              </span>
            </div>
            <div className="row">
              <span>Версия</span>
              <span>{info.version}</span>
            </div>
            <div className="row">
              <span>База</span>
              <span className={info.encrypted ? '' : 'warn'}>{info.encrypted ? `зашифрована, SQLCipher ${info.cipher}` : 'не зашифрована'}</span>
            </div>
            <div className="row">
              <span>Записей в журнале</span>
              <span>{info.events.toLocaleString('ru-RU')}</span>
            </div>
            <p className="note" style={{ overflowWrap: 'anywhere', marginTop: 6 }}>
              Папка данных: {info.data_dir}
            </p>
          </>
        ) : (
          <p className="note">Загружаю…</p>
        )}
      </section>

      <section className="stack gap10">
        <button type="button" className="collapse" aria-expanded={showChecks} onClick={() => setShowChecks(!showChecks)}>
          <span>Проверки</span>
          <Icon name={showChecks ? 'up' : 'down'} size={18} stroke={2} />
        </button>
        {showChecks && (
          <div className="stack gap16">
            <p className="note">Проверочная база: запись, чтение, шифрование файла. Расчёт: суммы из заявки 573 должны совпасть с 1С до копейки.</p>
            <CheckButton label="Проверить базу" run={api.dbCheck} />
            <CheckButton label="Проверить расчёт" run={api.calcCheck} />
          </div>
        )}
      </section>
    </div>
  );
}

function FxRow({ cur, value, save }: { cur: string; value: number | undefined; save: (v: string) => Promise<Snapshot | null> }) {
  const shown = value ? String(value).replace('.', ',') : '';
  const [text, setText] = useState(shown);
  useEffect(() => setText(shown), [shown]);
  const [error, setError] = useState(false);
  const commit = async () => {
    const t = text.trim().replace(',', '.');
    if (!t) return;
    const v = Number(t);
    if (!(v > 0) || !Number.isFinite(v)) return setError(true);
    setError(false);
    if (v !== value) await save(String(v));
  };
  return (
    <label className="setrow" style={{ minHeight: 52 }}>
      <span>
        <span>
          1 {cur} {SIGN[cur] ? `(${SIGN[cur]})` : ''}
        </span>
        {error && <span className="note warn">Введите число больше нуля</span>}
      </span>
      <span style={{ display: 'flex', alignItems: 'center', gap: 6, flex: 'none' }}>
        <input
          className="field num"
          inputMode="decimal"
          aria-label={`Курс ${cur}, рублей`}
          value={text}
          placeholder="нет"
          onChange={(e) => setText(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => e.key === 'Enter' && (e.target as HTMLInputElement).blur()}
          style={{ width: 96, height: 40, textAlign: 'right', fontSize: 16 }}
        />
        <span className="note">₽</span>
      </span>
    </label>
  );
}
