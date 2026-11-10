import { useEffect, useState } from 'react';
import { api, LOCKED, message, onBackup, type BackupInfo, type BackupStatus, type Snapshot } from '../api';
import { NB, plural } from '../format';
import { pickFiles } from '../media';
import { ErrorText, Icon, Seg } from '../ui';

type Run = (p: Promise<Snapshot>, ok?: string) => Promise<Snapshot | null>;

const MONTHS = ['января', 'февраля', 'марта', 'апреля', 'мая', 'июня', 'июля', 'августа', 'сентября', 'октября', 'ноября', 'декабря'];

/** «сегодня в 18:30», «вчера в 9:05», «7 ноября в 18:30», «7 ноября 2025 в 18:30». */
export function whenText(unix: number, now = Date.now()): string {
  const d = new Date(unix * 1000);
  const t = new Date(now);
  const time = `${d.getHours()}:${String(d.getMinutes()).padStart(2, '0')}`;
  const day = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const diff = Math.round((day(t) - day(d)) / 86400000);
  if (diff === 0) return `сегодня в ${time}`;
  if (diff === 1) return `вчера в ${time}`;
  const date = `${d.getDate()}${NB}${MONTHS[d.getMonth()]}` + (d.getFullYear() === t.getFullYear() ? '' : ` ${d.getFullYear()}`);
  return `${date} в ${time}`;
}

/** «12,4 МБ», «830 КБ». */
export function bytesText(bytes: number): string {
  if (bytes >= 1 << 30) return (bytes / (1 << 30)).toFixed(1).replace('.', ',') + NB + 'ГБ';
  if (bytes >= 1 << 20) return (bytes / (1 << 20)).toFixed(1).replace('.', ',') + NB + 'МБ';
  return Math.max(1, Math.round(bytes / 1024)) + NB + 'КБ';
}

/** Последняя копия по общей настройке «backup.last»: когда и на каком устройстве. Видна на всех устройствах. */
export function lastBackup(snap: Snapshot): { at: number; device: string } | null {
  const v = snap.settings['backup.last'];
  if (!v) return null;
  const i = v.indexOf('|');
  const at = Number(i < 0 ? v : v.slice(0, i));
  return Number.isFinite(at) && at > 0 ? { at, device: i < 0 ? '' : v.slice(i + 1) } : null;
}

/** Сколько полных дней прошло с последней копии. */
export function daysSince(unix: number, now = Date.now()): number {
  return Math.floor((now / 1000 - unix) / 86400);
}

const count = (n: number, one: string, few: string, many: string) => `${n.toLocaleString('ru-RU')}${NB}${plural(n, one, few, many)}`;

/** Файл копии: на телефоне у него нет известного системе типа — показываем все файлы. */
const pickBackup = async (mobile: boolean): Promise<File | null> => (await pickFiles({ accept: mobile ? '' : '.karman' }))[0] ?? null;

/** Загрузка выбранного файла, пароль, что в копии и восстановление — на компьютере и на телефоне. */
function Restore({ first, mobile, run, onClose }: { first: File; mobile: boolean; run: Run; onClose: () => void }) {
  const [file, setFile] = useState(first);
  const [step, setStep] = useState<'upload' | 'password' | 'check' | 'preview' | 'restore'>('upload');
  const [id, setId] = useState<string | null>(null);
  const [done, setDone] = useState(0);
  const [password, setPassword] = useState('');
  const [info, setInfo] = useState<BackupInfo | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    setStep('upload');
    setInfo(null);
    setDone(0);
    setError(null);
    api.backupUpload(file, (d) => alive && setDone(d)).then(
      (got) => {
        // Выбрали другой файл, пока этот загружался, — его загрузка не нужна.
        if (!alive) return void api.backupDiscard(got).catch(() => undefined);
        setId(got);
        setStep('password');
      },
      (e) => alive && setError(message(e)),
    );
    return () => {
      alive = false;
    };
  }, [file]);

  const discard = () => {
    if (id) api.backupDiscard(id).catch(() => undefined);
  };
  const cancel = () => {
    discard();
    onClose();
  };
  const another = async () => {
    const f = await pickBackup(mobile);
    if (!f) return;
    discard();
    setId(null);
    setFile(f);
  };
  const check = async () => {
    if (!id) return;
    setError(null);
    setStep('check');
    try {
      setInfo(await api.backupInspect(id, password));
      setStep('preview');
    } catch (e) {
      setStep('password');
      setError(message(e) === LOCKED ? 'Сначала войдите в «Карман»' : message(e));
    }
  };
  const restore = async () => {
    if (!id) return;
    setError(null);
    setStep('restore');
    try {
      const r = await api.backupRestore(id, password);
      const parts: string[] = [];
      if (r.ops) parts.push(count(r.ops, 'операция', 'операции', 'операций'));
      if (r.trips) parts.push(count(r.trips, 'поездка', 'поездки', 'поездок'));
      if (r.blobs) parts.push(count(r.blobs, 'снимок чека', 'снимка чеков', 'снимков чеков'));
      const text =
        r.events === 0 && r.blobs === 0 ? 'Всё из копии уже было на устройстве' : 'Восстановлено: ' + (parts.length ? parts.join(', ') : count(r.events, 'запись', 'записи', 'записей'));
      await run(Promise.resolve(r.snapshot), text);
      setPassword('');
      onClose();
    } catch (e) {
      setStep('preview');
      setError(message(e));
    }
  };

  return (
    <div className="card stack gap10 restore" role="group" aria-label="Восстановление из копии">
      <div className="between" style={{ alignItems: 'center' }}>
        <span style={{ fontWeight: 500, minWidth: 0, overflowWrap: 'anywhere' }}>{file.name}</span>
        <button type="button" className="iconbtn" aria-label="Закрыть восстановление" onClick={cancel} disabled={step === 'restore'}>
          <Icon name="x" size={20} stroke={2} />
        </button>
      </div>
      {step === 'upload' && !error && (
        <div className="stack gap6">
          <span className="bar" aria-hidden="true">
            <i style={{ width: `${file.size ? Math.round((done / file.size) * 100) : 0}%` }} />
          </span>
          <span className="note">
            Загружаю файл: {bytesText(done)} из {bytesText(file.size)}
          </span>
        </div>
      )}
      {(step === 'password' || step === 'check') && (
        <form
          className="stack gap10"
          onSubmit={(e) => {
            e.preventDefault();
            check();
          }}
        >
          <label className="stack gap6">
            <span className="note">Пароль этой копии</span>
            <input className="field" type="password" autoComplete="current-password" aria-label="Пароль копии" value={password} onChange={(e) => setPassword(e.target.value)} disabled={step === 'check'} autoFocus />
          </label>
          <button type="submit" className="btn" style={{ alignSelf: 'flex-start' }} disabled={!password || step === 'check'}>
            {step === 'check' ? 'Проверяю копию…' : 'Открыть копию'}
          </button>
        </form>
      )}
      {(step === 'preview' || step === 'restore') && info && (
        <div className="stack gap10">
          <div className="stack">
            <span>
              Копия от {whenText(info.created)}
              {info.device ? `, «${info.device}»` : ''}
            </span>
            <span className="note">
              {[count(info.ops, 'операция', 'операции', 'операций'), count(info.trips, 'поездка', 'поездки', 'поездок'), count(info.blobs, 'снимок чека', 'снимка чеков', 'снимков чеков'), bytesText(info.bytes)].join(' · ')}
              {info.app ? ` · «Карман» ${info.app}` : ''}
            </span>
          </div>
          <p className="note">Записи из копии добавятся к нынешним: ничего не пропадёт и не задвоится. То, что удалили или поправили уже после копии, так и останется.</p>
          <div className="wrap">
            <button type="button" className="btn own" onClick={restore} disabled={step === 'restore'}>
              {step === 'restore' ? 'Восстанавливаю…' : 'Добавить к данным'}
            </button>
            <button type="button" className="btn ghost" onClick={cancel} disabled={step === 'restore'}>
              Отмена
            </button>
          </div>
        </div>
      )}
      <ErrorText text={error} />
      {(step === 'upload' || step === 'password') && (error || step === 'password') && (
        <button type="button" className="btn link" style={{ alignSelf: 'flex-start', color: 'var(--own)', padding: 0, minHeight: 32 }} onClick={another}>
          Выбрать другой файл
        </button>
      )}
    </div>
  );
}

const EVERY: { id: BackupStatus['every']; label: string }[] = [
  { id: 'day', label: 'Каждый день' },
  { id: 'week', label: 'Раз в неделю' },
  { id: 'off', label: 'Выключены' },
];

/** Пароль копии: два поля, не короче восьми символов. */
function PasswordForm({ first, cloud, onSaved, onCancel }: { first: boolean; cloud: boolean; onSaved: (s: BackupStatus) => void; onCancel?: () => void }) {
  const [a, setA] = useState('');
  const [b, setB] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const save = async () => {
    if ([...a].length < 8) return setError('Пароль копии — не короче 8 символов');
    if (a !== b) return setError('Пароли не совпадают');
    setBusy(true);
    setError(null);
    try {
      const s = await api.backupPassword(a);
      setA('');
      setB('');
      onSaved(s);
    } catch (e) {
      setError(message(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <form
      className="stack gap10"
      style={{ paddingTop: 10 }}
      onSubmit={(e) => {
        e.preventDefault();
        save();
      }}
    >
      <p className="note">
        Не короче 8 символов, лучше — фраза из нескольких слов. Пароль нигде не хранится и не восстанавливается: забудете — копию не открыть.
        {first ? '' : ' Прежние копии открываются прежним паролем.'}
        {cloud ? ' Этим же паролем зашифровано облако: новый пароль уйдёт туда при ближайшем обмене.' : ''}
      </p>
      <input className="field" type="password" autoComplete="new-password" aria-label="Новый пароль копии" placeholder="Пароль копии" value={a} onChange={(e) => setA(e.target.value)} />
      <input className="field" type="password" autoComplete="new-password" aria-label="Пароль копии ещё раз" placeholder="Ещё раз" value={b} onChange={(e) => setB(e.target.value)} />
      <ErrorText text={error} />
      <div className="wrap">
        <button type="submit" className="btn" disabled={busy || !a || !b}>
          {busy ? 'Сохраняю…' : first ? 'Задать пароль' : 'Сменить пароль'}
        </button>
        {onCancel && (
          <button type="button" className="btn ghost" onClick={onCancel} disabled={busy}>
            Отмена
          </button>
        )}
      </div>
    </form>
  );
}

/** Настройки → «Резервная копия». */
export default function BackupSection({ snap, mobile, run, flash }: { snap: Snapshot; mobile: boolean; run: Run; flash: (text: string, long?: boolean) => void }) {
  const [st, setSt] = useState<BackupStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [changing, setChanging] = useState(false);
  // Выбранный файл копии: окно выбора открывается прямо по нажатию (иначе система его не покажет).
  const [restoring, setRestoring] = useState<File | null>(null);
  const startRestore = async () => {
    const f = await pickBackup(mobile);
    if (f) setRestoring(f);
  };
  const [making, setMaking] = useState(false);
  const load = () => api.backupStatus().then(setSt, (e) => message(e) !== LOCKED && setError(message(e)));
  useEffect(() => {
    load();
    const un = onBackup(load).catch(() => () => undefined);
    return () => {
      un.then((f) => f());
    };
  }, []);
  const shared = lastBackup(snap);
  const act = async (p: Promise<BackupStatus>) => {
    setError(null);
    try {
      setSt(await p);
    } catch (e) {
      setError(message(e));
    }
  };
  const now = async () => {
    setMaking(true);
    setError(null);
    try {
      const last = await api.backupNow();
      flash('Копия сохранена и проверена: ' + last.path, true);
      load();
      // Когда сделана копия — в общих настройках: обновить сводку (напоминание в обзоре).
      run(api.snapshot());
    } catch (e) {
      setError(message(e));
      load();
    } finally {
      setMaking(false);
    }
  };
  const desktop = st ? st.desktop : !mobile;

  return (
    <section className="stack" id="backup">
      <h2 className="h2" style={{ marginBottom: 6 }}>
        Резервная копия
      </h2>
      {desktop ? (
        <>
          <p className="note" style={{ marginBottom: 8 }}>
            Все записи и снимки чеков в одном файле, зашифрованном паролем копии. Файл открывается на любом компьютере или телефоне с «Карманом» — по паролю.
          </p>
          <div className="row" style={{ alignItems: 'baseline' }}>
            <span>Последняя копия</span>
            <span style={{ textAlign: 'right' }}>
              {st?.last ? (
                <>
                  {whenText(st.last.at)}
                  <span className="note" style={{ display: 'block' }}>
                    {(st.last.auto ? 'автокопия' : 'по кнопке') + ', ' + bytesText(st.last.bytes)}
                  </span>
                </>
              ) : shared ? (
                `${whenText(shared.at)}${shared.device ? `, «${shared.device}»` : ''}`
              ) : (
                <span className="warn">ещё не было</span>
              )}
            </span>
          </div>
          {st?.error && <p className="note warn">Автокопия не получилась: {st.error}</p>}

          <div className="setrow" style={{ cursor: 'default' }}>
            <span>
              <span>Пароль копии</span>
              <span className={'note' + (st && !st.password ? ' warn' : '')}>{st ? (st.password ? (st.cloud ? 'Задан, им же зашифровано облако' : 'Задан') : 'Не задан — без него копии не делаются') : '…'}</span>
            </span>
            {st?.password && !changing && (
              <button type="button" className="btn ghost" style={{ flex: 'none' }} onClick={() => setChanging(true)}>
                Сменить
              </button>
            )}
          </div>
          {st && (!st.password || changing) && (
            <PasswordForm
              first={!st.password}
              cloud={st.cloud}
              onSaved={(s) => {
                setSt(s);
                setChanging(false);
                flash(st.password ? 'Пароль копии сменён' : 'Пароль копии задан: копии будут делаться сами');
              }}
              onCancel={st.password ? () => setChanging(false) : undefined}
            />
          )}

          <div className="setrow" style={{ cursor: 'default', alignItems: 'flex-start' }}>
            <span>
              <span>Папка</span>
              <span className="note" style={{ overflowWrap: 'anywhere' }}>
                {st?.dir ?? '…'}
              </span>
              {st && !st.chosen && <span className="note">По умолчанию. Выберите папку Яндекс Диска или Google Drive на компьютере — копии сами уйдут в облако.</span>}
              {st?.chosen && (
                <button type="button" className="btn link" style={{ alignSelf: 'flex-start', color: 'var(--own)', padding: 0, minHeight: 32 }} onClick={() => act(api.backupDir(null))}>
                  Вернуть папку по умолчанию
                </button>
              )}
            </span>
            <button type="button" className="btn ghost" style={{ flex: 'none', marginTop: 6 }} onClick={() => act(api.backupPickDir())}>
              Выбрать папку
            </button>
          </div>

          <div className="stack gap6" style={{ marginTop: 10 }}>
            <span className="note">Автокопии</span>
            <Seg label="Как часто делать автокопию" value={st?.every ?? 'day'} onPick={(v) => act(api.backupEvery(v))} items={EVERY} />
            <p className="note">
              {st?.every === 'off'
                ? 'Только по кнопке «Сделать копию».'
                : `${st?.every === 'week' ? 'Раз в неделю' : 'Раз в день'}, пока «Карман» открыт или в трее. Хранятся последние ${st?.keep ?? 14}, старые удаляются сами.`}
            </p>
          </div>

          <div className="wrap" style={{ marginTop: 12 }}>
            <button type="button" className="btn" disabled={!st?.password || making || st?.running} onClick={now}>
              {making || st?.running ? 'Делаю копию…' : 'Сделать копию'}
            </button>
            {!restoring && (
              <button type="button" className="btn ghost" onClick={startRestore}>
                Восстановить из копии
              </button>
            )}
          </div>
        </>
      ) : (
        <>
          <p className="note" style={{ marginBottom: 8 }}>
            Копии делает «Карман» на компьютере: задайте там пароль копии. Записи телефона попадают в копию после обмена по Wi-Fi или через облако, а новый телефон получит всё обратно из облака или от компьютера.
          </p>
          <div className="row" style={{ alignItems: 'baseline' }}>
            <span>Последняя копия</span>
            <span style={{ textAlign: 'right' }}>{shared ? `${whenText(shared.at)}${shared.device ? `, «${shared.device}»` : ''}` : <span className="warn">ещё не было</span>}</span>
          </div>
          {!restoring && (
            <div className="setrow" style={{ cursor: 'default' }}>
              <span>
                <span>Восстановить из файла</span>
                <span className="note">Если компьютера нет под рукой: скачайте файл копии из облака на телефон</span>
              </span>
              <button type="button" className="btn ghost" style={{ flex: 'none' }} onClick={startRestore}>
                Выбрать файл
              </button>
            </div>
          )}
        </>
      )}
      {restoring && (
        <div style={{ marginTop: 12 }}>
          <Restore key={restoring.name + restoring.size + restoring.lastModified} first={restoring} mobile={mobile} run={run} onClose={() => setRestoring(null)} />
        </div>
      )}
      <ErrorText text={error} />
    </section>
  );
}
