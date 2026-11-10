import { useCallback, useEffect, useState } from 'react';
import { api, LOCKED, message, onCloud, type CloudLast, type CloudStatus, type CloudStep } from '../api';
import { NB, plural } from '../format';
import { ErrorText, Icon, Seg } from '../ui';
import { whenText } from './Backup';

const count = (n: number, one: string, few: string, many: string) => `${n.toLocaleString('ru-RU')}${NB}${plural(n, one, few, many)}`;

const EVERY: { id: CloudStatus['every']; label: string }[] = [
  { id: 'auto', label: 'Сам' },
  { id: 'manual', label: 'Только по кнопке' },
];

export function stepText(s: CloudStep | null): string {
  if (!s || s.step === 'events') return 'Обмениваюсь записями…';
  const n = `${Math.min(s.done + 1, s.total)} из ${s.total}`;
  return s.step === 'upload' ? `Отправляю снимки чеков: ${n}` : `Получаю снимки чеков: ${n}`;
}

export function lastText(l: CloudLast): string {
  const parts = [`получено ${l.received}, отправлено ${l.sent}`];
  if (l.files_received + l.files_sent > 0) parts.push(`снимков чеков: получено ${l.files_received}, отправлено ${l.files_sent}`);
  return parts.join('; ');
}

/** Пароль копий для облака: ввести (облако уже есть или пароль копий задан) или придумать. */
function CloudPassword({ st, onDone, onCancel }: { st: CloudStatus; onDone: (s: CloudStatus) => void; onCancel: () => void }) {
  const invent = !st.cloud_exists && !(st.desktop && st.backup_password);
  const [a, setA] = useState('');
  const [b, setB] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const save = async () => {
    if ([...a].length < 8) return setError('Пароль копий — не короче 8 символов');
    if (invent && a !== b) return setError('Пароли не совпадают');
    setBusy(true);
    setError(null);
    try {
      const s = await api.cloudPassword(a);
      setA('');
      setB('');
      onDone(s);
    } catch (e) {
      setError(message(e));
    } finally {
      setBusy(false);
    }
  };
  const text = st.cloud_exists
    ? 'В облаке уже есть данные «Кармана». Введите пароль копий, которым они зашифрованы.'
    : st.desktop && st.backup_password
      ? 'Введите пароль копий этого компьютера — им будут зашифрованы данные в облаке.'
      : `Придумайте пароль копий: им шифруются данные в облаке${st.desktop ? ' и резервные копии' : ''}. Не короче 8 символов, лучше — фраза из нескольких слов. Запишите его: без пароля новое устройство не получит данные из облака.`;
  return (
    <form
      className="stack gap10"
      onSubmit={(e) => {
        e.preventDefault();
        save();
      }}
    >
      <p className="note">{text}</p>
      <input
        className="field"
        type="password"
        autoComplete={invent ? 'new-password' : 'current-password'}
        aria-label="Пароль копий"
        placeholder="Пароль копий"
        value={a}
        onChange={(e) => setA(e.target.value)}
        disabled={busy}
      />
      {invent && <input className="field" type="password" autoComplete="new-password" aria-label="Пароль копий ещё раз" placeholder="Ещё раз" value={b} onChange={(e) => setB(e.target.value)} disabled={busy} />}
      <ErrorText text={error} />
      <div className="wrap">
        <button type="submit" className="btn own" disabled={busy || !a || (invent && !b)}>
          {busy ? 'Проверяю пароль…' : 'Подключить'}
        </button>
        <button type="button" className="btn ghost" disabled={busy} onClick={onCancel}>
          Отмена
        </button>
      </div>
    </form>
  );
}

type DavKind = 'yadisk' | 'webdav';

/** Вход по логину и паролю: Яндекс Диск (пароль приложения) или другой сервер WebDAV. */
function DavForm({ kind, st, again, onDone, onCancel }: { kind: DavKind; st: CloudStatus; again: boolean; onDone: (s: CloudStatus) => void; onCancel: () => void }) {
  const ya = kind === 'yadisk';
  const [url, setUrl] = useState(st.dav && st.provider === kind ? st.dav.url : '');
  const [user, setUser] = useState(st.dav && st.provider === kind ? st.dav.user : '');
  const [password, setPassword] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      const s = await api.cloudDavLogin(kind, ya ? null : url, user, password);
      setPassword('');
      onDone(s);
    } catch (e) {
      setError(message(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <form
      className="stack gap10"
      aria-label={ya ? 'Вход в Яндекс Диск' : 'Вход на сервер WebDAV'}
      onSubmit={(e) => {
        e.preventDefault();
        submit();
      }}
    >
      {ya ? (
        <>
          <p className="note">
            Нужен <b>пароль приложения</b>, а не пароль от аккаунта Яндекса: Яндекс ID → Безопасность → Доступ к вашим данным → Пароли приложений → «Файлы WebDAV». Яндекс показывает его один раз — сразу скопируйте. Новый пароль начинает действовать через 2–3{NB}часа.
          </p>
          <div className="wrap">
            <button type="button" className="btn ghost" onClick={() => api.cloudAppPasswords().catch((e) => setError(message(e)))}>
              Открыть Яндекс ID
            </button>
          </div>
        </>
      ) : (
        <>
          <p className="note">Подойдёт сервер WebDAV с https: Nextcloud, Synology или другой NAS. Адрес — папка WebDAV пользователя, например https://cloud.example.ru/remote.php/dav/files/имя.</p>
          <input className="field" type="url" inputMode="url" autoComplete="url" spellCheck={false} aria-label="Адрес сервера" placeholder="https://…" value={url} onChange={(e) => setUrl(e.target.value)} disabled={busy} />
        </>
      )}
      <input
        className="field"
        autoComplete="username"
        autoCapitalize="none"
        spellCheck={false}
        aria-label={ya ? 'Логин Яндекса' : 'Логин'}
        placeholder={ya ? 'Логин Яндекса' : 'Логин'}
        value={user}
        onChange={(e) => setUser(e.target.value)}
        disabled={busy}
      />
      <input
        className="field"
        type="password"
        autoComplete="current-password"
        aria-label={ya ? 'Пароль приложения' : 'Пароль'}
        placeholder={ya ? 'Пароль приложения' : 'Пароль'}
        value={password}
        onChange={(e) => setPassword(e.target.value)}
        disabled={busy}
      />
      <p className="note faint">Пароль хранится только на этом устройстве, зашифрованным, и не попадает в копии. «Карман» работает только в своей папке «Карман».</p>
      <ErrorText text={error} />
      <div className="wrap">
        <button type="submit" className="btn own" disabled={busy || !user.trim() || !password || (!ya && !url.trim())}>
          {busy ? 'Проверяю вход…' : again ? 'Войти снова' : 'Войти'}
        </button>
        {!again && (
          <button type="button" className="btn ghost" disabled={busy} onClick={onCancel}>
            Отмена
          </button>
        )}
      </div>
    </form>
  );
}

/** Настройки → Синхронизация → «Личное облако»: Яндекс Диск или другой сервер WebDAV (вход по паролю), по коду — если есть регистрация. */
export default function CloudSync() {
  const [st, setSt] = useState<CloudStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [asking, setAsking] = useState(false);
  const [wipe, setWipe] = useState(false);
  const [copied, setCopied] = useState(false);
  const [form, setForm] = useState<DavKind | null>(null);

  const load = useCallback(() => api.cloudStatus().then(setSt, (e) => message(e) !== LOCKED && setError(message(e))), []);
  useEffect(() => {
    load();
    const un = onCloud(() => load()).catch(() => () => undefined);
    return () => {
      un.then((f) => f());
    };
  }, [load]);

  // Пока виден код входа, каждые 2 секунды спрашиваем, ввели ли его.
  const signin = st?.state === 'signin';
  useEffect(() => {
    if (!signin) return;
    let alive = true;
    const id = window.setInterval(async () => {
      try {
        const s = await api.cloudLoginCheck();
        if (alive) setSt(s);
      } catch (e) {
        if (!alive) return;
        setError(message(e));
        load();
      }
    }, 2000);
    return () => {
      alive = false;
      window.clearInterval(id);
    };
  }, [signin, load]);

  const act = async (p: () => Promise<CloudStatus>): Promise<CloudStatus | null> => {
    setBusy(true);
    setError(null);
    try {
      const s = await p();
      setSt(s);
      setNotice(s.notice);
      return s;
    } catch (e) {
      setError(message(e));
      return null;
    } finally {
      setBusy(false);
    }
  };
  const copy = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch {
      /* код виден на экране — его можно набрать */
    }
  };
  const disconnect = async () => {
    const s = await act(() => api.cloudDisconnect(wipe));
    if (s) {
      setAsking(false);
      setWipe(false);
    }
  };

  const who = st?.dav ? (st.provider === 'webdav' ? st.dav.host : st.dav.user) : null;
  const subtitle = !st || st.state === 'off' ? 'Яндекс Диск или свой сервер' : st.state === 'ready' ? `${st.title} подключён${who ? ` · ${who}` : ''}` : (st.title ?? st.login?.title ?? '');
  const davLink = st?.provider === 'yadisk' || st?.provider === 'webdav' ? (st.provider as DavKind) : null;
  const signedIn = (s: CloudStatus) => {
    setSt(s);
    setNotice(s.notice);
    setForm(null);
  };

  return (
    <div className="stack" id="cloud">
      <div className="setrow" style={{ minHeight: 58, marginTop: 14, cursor: 'default' }}>
        <span style={{ flexDirection: 'row', alignItems: 'center', gap: 12 }}>
          <Icon name="cloud" size={22} />
          <span className="stack">
            <span>Личное облако</span>
            <span className="note">{subtitle}</span>
          </span>
        </span>
      </div>
      <div className="stack gap10" style={{ paddingTop: 8 }}>
        {st?.state === 'off' && !form && (
          <>
            <p className="note">
              Записи и снимки чеков ходят через ваш Яндекс Диск или свой сервер — в командировке общий Wi-Fi не нужен. В облаке лежат только зашифрованные файлы в папке «Карман»: открыть их можно только паролем копий.
            </p>
            <div className="wrap">
              {st.providers.map((p) => (
                <button
                  key={p.code}
                  type="button"
                  className={'btn' + (p.code === 'yadisk' ? ' own' : '')}
                  disabled={busy}
                  onClick={() => (p.kind === 'password' ? (setError(null), setNotice(null), setForm(p.code as DavKind)) : act(() => api.cloudLogin(p.code)))}
                >
                  {p.title}
                </button>
              ))}
            </div>
          </>
        )}

        {st?.state === 'off' && form && <DavForm key={form} kind={form} st={st} again={false} onDone={signedIn} onCancel={() => setForm(null)} />}

        {st?.state === 'signin' && st.login && (
          <>
            <p>Войдите в аккаунт {st.login.provider === 'yandex' ? 'Яндекса' : 'Google'} в браузере и введите код:</p>
            <div className="logincode" aria-label="Код для входа">
              {st.login.user_code}
            </div>
            <div className="wrap">
              <button type="button" className="btn own" onClick={() => copy(st.login!.user_code)}>
                {copied ? 'Скопировано' : 'Скопировать код'}
              </button>
              <button type="button" className="btn ghost" onClick={() => api.cloudOpenPage().catch((e) => setError(message(e)))}>
                Открыть страницу
              </button>
              <button type="button" className="btn ghost" disabled={busy} onClick={() => act(api.cloudLoginCancel)}>
                Отмена
              </button>
            </div>
            <p className="note">
              Страница входа: <span style={{ overflowWrap: 'anywhere' }}>{st.login.url}</span>. «Карман» получит доступ только к своей папке
              {st.login.provider === 'yandex' ? ' «Приложения/Карман»' : ' «Карман»'}. Код действует {Math.max(1, Math.round(st.login.seconds_left / 60))}
              {NB}мин.
            </p>
            <p className="note faint">Жду, пока вы введёте код…</p>
          </>
        )}

        {st?.state === 'password' && (
          <>
            {st.error && <p className="note warn">{st.error.text}</p>}
            <p className="note">
              {st.title}: вход выполнен{who ? ` (${who})` : ''}.
            </p>
            <CloudPassword
              st={st}
              onDone={(s) => {
                setSt(s);
                setNotice(s.notice);
              }}
              onCancel={() => act(() => api.cloudDisconnect(false)).then(() => setNotice(null))}
            />
          </>
        )}

        {st?.state === 'relogin' && davLink && (
          <>
            <p className="note warn">{st.error?.text ?? `${st.title} не принимает пароль — войдите снова.`}</p>
            <p className="note">Данные и пароль копий остаются на устройстве: после входа обмен продолжится сам.</p>
            <DavForm key={davLink} kind={davLink} st={st} again onDone={signedIn} onCancel={() => undefined} />
            <div className="wrap">
              <button type="button" className="btn ghost" disabled={busy} onClick={() => setAsking(true)}>
                Отключить облако
              </button>
            </div>
          </>
        )}

        {st?.state === 'relogin' && !davLink && (
          <>
            <p className="note warn">{st.error?.text ?? `Вход в облако устарел — войдите снова.`}</p>
            <p className="note">Данные и пароль копий остаются на устройстве: после входа обмен продолжится сам.</p>
            <div className="wrap">
              <button type="button" className="btn own" disabled={busy} onClick={() => act(() => api.cloudLogin(st.provider!))}>
                Войти снова
              </button>
              <button type="button" className="btn ghost" disabled={busy} onClick={() => setAsking(true)}>
                Отключить
              </button>
            </div>
          </>
        )}

        {st?.state === 'ready' && (
          <>
            <div className="row" style={{ alignItems: 'baseline' }}>
              <span>Последний обмен</span>
              <span style={{ textAlign: 'right' }} aria-live="polite">
                {st.running ? (
                  stepText(st.progress)
                ) : st.last ? (
                  <>
                    {whenText(st.last.at)}
                    <span className="note" style={{ display: 'block' }}>
                      {lastText(st.last)}
                    </span>
                  </>
                ) : (
                  <span className="note">ещё не было</span>
                )}
              </span>
            </div>
            {!st.running && st.last && st.last.files_waiting > 0 && (
              <p className="note">Ещё {count(st.last.files_waiting, 'снимок чека придёт', 'снимка чеков придут', 'снимков чеков придут')}, когда их отправит другое устройство.</p>
            )}
            {st.error && <p className="note warn">{st.error.text}</p>}
            {st.last?.problems.map((p) => (
              <p key={p} className="note faint">
                {p}
              </p>
            ))}
            {st.key_pending && <p className="note">Новый пароль копий уйдёт в облако при ближайшем обмене.</p>}
            <div className="stack gap6" style={{ marginTop: 4 }}>
              <span className="note">Обмен через облако</span>
              <Seg label="Когда обмениваться через облако" value={st.every} onPick={(v) => act(() => api.cloudEvery(v))} items={EVERY} />
              <p className="note">{st.every === 'auto' ? 'При открытии «Кармана», через минуту после изменений и каждые 15 минут, пока он открыт.' : 'Только по кнопке «Обменяться сейчас».'}</p>
            </div>
            <div className="wrap">
              <button type="button" className="btn own" disabled={busy || st.running} onClick={() => act(api.cloudSyncNow)}>
                {st.running ? 'Обмениваюсь…' : 'Обменяться сейчас'}
              </button>
              <button type="button" className="btn ghost" disabled={busy} onClick={() => setAsking(true)}>
                Отключить
              </button>
            </div>
          </>
        )}

        {asking && st && (st.state === 'ready' || st.state === 'relogin') && (
          <div className="stack gap10 confirmbox" role="group" aria-label="Отключить облако">
            <p>Отключить облако на этом устройстве?</p>
            {st.state === 'ready' && (
              <label className="wrap" style={{ alignItems: 'flex-start', gap: 10, flexWrap: 'nowrap' }}>
                <input type="checkbox" checked={wipe} onChange={(e) => setWipe(e.target.checked)} style={{ flex: 'none', width: 20, height: 20, margin: 0, accentColor: 'var(--own)' }} />
                <span className="stack">
                  <span>Удалить данные «Кармана» из облака</span>
                  <span className="note">Другие устройства тоже перестанут обмениваться через облако. На самих устройствах всё останется.</span>
                </span>
              </label>
            )}
            <div className="wrap">
              <button type="button" className={'btn' + (wipe ? ' warn' : '')} disabled={busy} onClick={disconnect}>
                {busy ? 'Отключаю…' : wipe ? 'Отключить и удалить' : 'Отключить'}
              </button>
              <button
                type="button"
                className="btn ghost"
                disabled={busy}
                onClick={() => {
                  setAsking(false);
                  setWipe(false);
                }}
              >
                Отмена
              </button>
            </div>
          </div>
        )}

        {notice && <p className="note">{notice}</p>}
        <ErrorText text={error} />
      </div>
    </div>
  );
}
