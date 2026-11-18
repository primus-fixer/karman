import { useEffect, useState } from 'react';
import { api, message, type Account, type AccountView, type Check, type DeviceInfo, type LockStatus, type RemindStatus, type Snapshot } from '../api';
import { Row, SCHEMES, type PinMode, type ThemeMode } from '@primus-fixer/kit/ui';
import { SIGN, amountText, dMon, parseAmount, rateText as fxText, rub } from '../format';
import { Checks, Cut, ErrorText, Icon, Seg } from '../ui';
import AiSection from './Ai';
import BackupSection from './Backup';
import CategoriesSection from './Categories';
import ClaudeSection from './Claude';
import CloudSync from './Cloud';
import Wifi from './Wifi';
import { OvertimeSettings } from './Overtime';

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
    <Cut id="security" title="Защита">
      <div className="setrow">
        <span>
          <span>Вход по PIN-коду</span>
          <span className="small muted">{lock.pin ? 'Спрашивается при запуске и после 5 минут в фоне' : 'Выключен: «Карман» открывается без кода'}</span>
        </span>
        {!lock.pin && (
          <button type="button" className="btn own" style={{ flex: 'none' }} onClick={() => sec.onPin('create')}>
            Задать PIN
          </button>
        )}
      </div>
      {lock.pin && (
        <div className="sec-actions" style={{ padding: '2px 0 12px' }}>
          <button type="button" className="btn line" onClick={() => sec.onPin('change')}>
            Сменить PIN
          </button>
          <button type="button" className="btn line" onClick={() => sec.onPin('off')}>
            Выключить
          </button>
          <button type="button" className="btn line" style={{ display: 'flex', alignItems: 'center', gap: 8 }} onClick={sec.onLockNow}>
            <Icon name="lock" size={18} />
            Заблокировать сейчас
          </button>
        </div>
      )}
      <label className="setrow" style={{ opacity: lock.pin && lock.bio_ready ? 1 : 0.65, cursor: lock.pin && lock.bio_ready ? 'pointer' : 'default' }}>
        <span>
          <span>{bioTitle}</span>
          <span className="small muted">{bioNote}</span>
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
      <Row label="Ключ базы">{keyText}</Row>
      {lock.pin && (
        <p className="small muted" style={{ marginTop: 6 }}>
          Забыли PIN-код — на экране входа нажмите «Забыли PIN?» и подтвердите себя {recover}. Данные останутся.
        </p>
      )}
    </Cut>
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
  { id: 'savings', label: 'Накопительный', note: 'Копилка или накопительный счёт: идёт в «Отложено», сюда откладывают на цели' },
  { id: 'deposit', label: 'Вклад', note: 'Ставка и срок: «Карман» покажет прогноз процентов и напомнит о конце срока' },
];

const CURRENCIES = ['RUB', 'KZT', 'USD', 'EUR'];

const LEADS: [string, string][] = [
  ['0', 'В день'],
  ['1', 'За 1 день'],
  ['3', 'За 3 дня'],
  ['7', 'За неделю'],
];

/** Напоминания о платежах и сроках долгов: за сколько дней, во сколько, как на этом устройстве. */
function Reminders({ snap, run }: { snap: Snapshot; run: (p: Promise<Snapshot>, ok?: string) => Promise<Snapshot | null> }) {
  const [st, setSt] = useState<RemindStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    api.remindStatus(false).then(setSt, () => undefined);
  }, []);
  const time = snap.settings['remind.time'] || '10:00';
  const [t, setT] = useState(time);
  useEffect(() => setT(time), [time]);
  const lead = LEADS.some(([id]) => id === String(snap.remind_days)) ? String(snap.remind_days) : '3';
  const pickDesktop = async (mode: 'open' | 'windows') => {
    if (!st) return;
    try {
      await api.remindDesktop(mode);
      setSt({ ...st, desktop: mode });
      setError(null);
    } catch (e) {
      setError(message(e));
    }
  };
  return (
    <Cut id="remind" title="Напоминания">
      <p className="small muted" style={{ marginBottom: 8 }}>
        О регулярных платежах и сроках долгов: заранее и ещё раз в сам день. В уведомлении только название, без суммы.
      </p>
      <Seg cols small label="За сколько дней напоминать" value={lead} onChange={(v) => run(api.settingSet('remind.days', v), v === '0' ? 'Напоминания в день платежа' : `Напоминания ${LEADS.find(([id]) => id === v)?.[1].toLowerCase()} и в день платежа`)} options={LEADS} />
      <label className="setrow" style={{ marginTop: 8 }}>
        <span>
          <span>Время напоминаний</span>
          <span className="small muted">У отдельного платежа «за сколько дней» можно поставить своё</span>
        </span>
        <input
          className="input num"
          type="time"
          aria-label="Время напоминаний"
          value={t}
          onChange={(e) => {
            // Поле времени сообщает об изменении, только когда время введено целиком.
            const v = e.target.value;
            setT(v);
            if (/^\d\d:\d\d$/.test(v) && v !== time) run(api.settingSet('remind.time', v), 'Напоминания в ' + v);
          }}
          style={{ flex: 'none', width: 112, height: 40, fontSize: 16 }}
        />
      </label>
      {st?.platform === 'android' && (
        <div className="setrow">
          <span>
            <span>Уведомления</span>
            <span className={'small muted' + (st.permission === 'denied' ? ' warn-text' : '')}>
              {st.permission === 'granted'
                ? 'Разрешены: напомнит, даже если «Карман» закрыт'
                : st.permission === 'denied'
                  ? 'Запрещены. Разрешите в настройках телефона: Приложения → Карман → Уведомления'
                  : 'Ещё не разрешены'}
            </span>
          </span>
          {st.permission !== 'granted' && (
            <button type="button" className="btn line" style={{ flex: 'none' }} onClick={() => api.remindStatus(true).then(setSt, (e) => setError(message(e)))}>
              Разрешить
            </button>
          )}
        </div>
      )}
      {st && st.platform !== 'android' && (
        <div className="stack gap6" style={{ marginTop: 10 }}>
          <span className="small muted">На этом компьютере</span>
          <Seg cols
            label="Напоминания на компьютере"
            value={st.desktop}
            onChange={pickDesktop}
            options={[
              ['open', 'При открытии'],
              ['windows', 'Уведомлением Windows'],
            ]}
          />
          <p className="small muted">
            {st.desktop === 'windows'
              ? '«Карман» запускается вместе с Windows и работает в трее: крестик прячет окно, выйти — в меню значка. В назначенное время придёт уведомление Windows.'
              : 'Напоминания покажет сам «Карман», когда вы его откроете.'}
          </p>
        </div>
      )}
      <ErrorText text={error} />
    </Cut>
  );
}

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
  const [rate, setRate] = useState(base.deposit ? String(base.deposit.rate).replace('.', ',') : '');
  const [opened, setOpened] = useState(base.deposit?.opened ?? snap.today);
  const [until, setUntil] = useState(base.deposit?.until ?? '');
  const [capitalize, setCapitalize] = useState(base.deposit?.capitalize ?? true);
  const [bal, setBal] = useState(view ? (view.balance < 0 ? '-' : '') + amountText(view.balance) : '');
  const [error, setError] = useState<string | null>(null);
  const save = async () => {
    if (!name.trim()) return setError('Укажите название счёта');
    const b = bal.trim() ? parseSigned(bal) : 0;
    if (b == null) return setError('Не получается прочитать остаток');
    let deposit = null;
    if (kind === 'deposit') {
      const r = Number(rate.trim().replace(',', '.'));
      if (!rate.trim() || !Number.isFinite(r) || r < 0 || r > 100) return setError('Ставка вклада — процентов годовых, например 16,5');
      if (until && until <= opened) return setError('Вклад заканчивается раньше, чем открыт');
      deposit = { rate: r, opened: opened || snap.today, until: until || null, capitalize };
    }
    const ok = await onSave({ ...base, name: name.trim(), short: short.trim(), currency, kind, archived, opening: b - moved, deposit });
    if (!ok) setError('Не удалось сохранить');
  };
  return (
    <div className="stack gap12" style={{ padding: '12px 0 16px', borderTop: '1px solid var(--line)' }}>
      <div className="form-grid">
        <label className="field">
          <span>Название</span>
          <input className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder="Например, Карта Сбера" />
        </label>
        <label className="field">
          <span>Коротко, для кнопок ввода</span>
          <input className="input" value={short} onChange={(e) => setShort(e.target.value)} placeholder="Сбер" maxLength={14} />
        </label>
        <label className="field">
          <span>Остаток сейчас, {SIGN[currency] || currency}</span>
          <input className="input num" inputMode="decimal" value={bal} onChange={(e) => setBal(e.target.value)} placeholder="0" />
        </label>
        <label className="field">
          <span>Валюта</span>
          <select className="input" value={currency} disabled={used} onChange={(e) => setCurrency(e.target.value)}>
            {CURRENCIES.concat(CURRENCIES.includes(currency) ? [] : [currency]).map((c) => (
              <option key={c} value={c}>
                {c} {SIGN[c] || ''}
              </option>
            ))}
          </select>
        </label>
      </div>
      {used && <p className="small muted">Валюту нельзя поменять: по счёту уже есть операции.</p>}
      <div className="chips" role="group" aria-label="Тип счёта">
        {KINDS.map((k) => (
          <button key={k.id} type="button" aria-pressed={k.id === kind} className={'chip' + (k.id === kind ? ' sel' : '')} onClick={() => setKind(k.id)}>
            {k.label}
          </button>
        ))}
      </div>
      <p className="small muted" style={{ marginTop: -6 }}>
        {KINDS.find((k) => k.id === kind)?.note}
      </p>
      {kind === 'deposit' && (
        <div className="stack gap10">
          <div className="form-grid">
            <label className="field">
              <span>Ставка, % годовых</span>
              <input className="input num" inputMode="decimal" value={rate} onChange={(e) => setRate(e.target.value)} placeholder="Например, 16,5" />
            </label>
            <label className="field">
              <span>Открыт</span>
              <input className="input" type="date" value={opened} onChange={(e) => setOpened(e.target.value)} />
            </label>
            <label className="field">
              <span>До (пусто — бессрочный)</span>
              <input className="input" type="date" value={until} onChange={(e) => setUntil(e.target.value)} />
            </label>
          </div>
          <label style={{ display: 'flex', alignItems: 'center', gap: 10, minHeight: 44 }}>
            <input type="checkbox" checked={capitalize} onChange={(e) => setCapitalize(e.target.checked)} style={{ width: 20, height: 20, margin: 0, accentColor: 'var(--own)' }} />
            <span>Капитализация: проценты каждый месяц прибавляются к вкладу</span>
          </label>
          <p className="small muted" style={{ marginTop: -6 }}>
            Проценты от банка записывайте доходом «Проценты» на этот счёт — прогноз пересчитается по новому остатку.
          </p>
        </div>
      )}
      {view && (
        <label style={{ display: 'flex', alignItems: 'center', gap: 10, minHeight: 44 }}>
          <input type="checkbox" checked={archived} onChange={(e) => setArchived(e.target.checked)} style={{ width: 20, height: 20, margin: 0, accentColor: 'var(--own)' }} />
          <span>Скрыть счёт: не показывать в обзоре и при вводе</span>
        </label>
      )}
      <ErrorText text={error} />
      <div className="sec-actions">
        <button type="button" className="btn own" onClick={save}>
          Сохранить
        </button>
        <button type="button" className="btn line" onClick={onCancel}>
          Отмена
        </button>
      </div>
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
        className="btn line"
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

/** «Обновить курс ЦБ»: курсы на сегодня заново, не дожидаясь следующего запуска. */
function FxRefresh({ run, flash }: { run: (p: Promise<Snapshot>, ok?: string) => Promise<Snapshot | null>; flash: (text: string, long?: boolean) => void }) {
  const [busy, setBusy] = useState(false);
  const refresh = async () => {
    setBusy(true);
    let date = '';
    const s = await run(
      api.fxRefresh().then((r) => {
        date = r.date;
        return r.snap;
      }),
    );
    setBusy(false);
    if (s && date) flash(`Курсы ЦБ на ${dMon(date)} загружены`);
  };
  return (
    <button type="button" className="btn line" style={{ alignSelf: 'flex-start', margin: '8px 0 2px' }} disabled={busy} onClick={refresh}>
      {busy ? 'Загружаю курсы ЦБ…' : 'Обновить курс ЦБ'}
    </button>
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
  flash,
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
  flash: (text: string, long?: boolean) => void;
}) {
  const [rateText, setRateText] = useState(amountText(snap.rate));
  useEffect(() => setRateText(amountText(snap.rate)), [snap.rate]);
  const [editAcc, setEditAcc] = useState<string | null>(null);
  const [rateError, setRateError] = useState<string | null>(null);

  const commitRate = async () => {
    const k = parseAmount(rateText);
    if (k == null || k > 10_000_000) return setRateError('Введите сумму в рублях, например 700');
    setRateError(null);
    if (k !== snap.rate) await run(api.settingSet('rate', String(k)), 'Ставка суточных: ' + rub(k) + ' в день');
  };

  const fxCurrencies = Array.from(new Set(['KZT', 'USD', 'EUR', ...snap.accounts.map((a) => a.account.currency)])).filter((c) => c !== 'RUB');

  return (
    <div className="stack gap0 settings">
      <h1 className="screen-title">Настройки</h1>

      <Cut id="trips" title="Командировки">
        <label className="setrow">
          <span>
            <span>Суточные в день, ₽</span>
            <span className="small muted">Для поездок, по которым отчёт ещё не сдан. В поездке можно задать свою ставку</span>
          </span>
          <input
            className="input num"
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
          <Row key={k} label={k}>
            {v}
          </Row>
        ))}
      </Cut>

      <OvertimeSettings snap={snap} run={run} />

      <Cut id="accounts" title="Счета">
        <p className="small muted" style={{ marginBottom: 8 }}>
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
              <span className="stack gap0" style={{ minWidth: 0 }}>
                <span>{a.account.name}</span>
                <span className="small muted">
                  {(KINDS.find((k) => k.id === a.account.kind)?.label ?? 'Счёт') + (a.account.archived ? ', скрыт' : '')}
                </span>
              </span>
              <span className="num" style={{ flex: 'none', textAlign: 'right' }}>
                {rub(a.balance, a.account.currency)}
                {a.account.currency !== 'RUB' && <span className="small muted muted3" style={{ display: 'block' }}>≈ {rub(a.balance_rub)}</span>}
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
          <button type="button" className="btn line" style={{ marginTop: 10, alignSelf: 'flex-start' }} onClick={() => setEditAcc('new')}>
            Добавить счёт
          </button>
        )}
        {snap.accounts.some((a) => !a.account.archived && (a.account.kind === 'savings' || a.account.kind === 'deposit')) && (
          <label className="setrow" style={{ cursor: 'pointer', marginTop: 10 }}>
            <span>
              <span>Считать отложенное в «Моих деньгах»</span>
              <span className="small muted">Накопительные счета и вклады. Выключено — они отдельной строкой «Отложено»</span>
            </span>
            <input
              type="checkbox"
              role="switch"
              aria-label="Считать отложенное в «Моих деньгах»"
              checked={snap.saved_in_mine}
              onChange={(e) => run(api.settingSet('savings.in_mine', e.target.checked ? '1' : '0'), e.target.checked ? 'Отложенное входит в «Мои деньги»' : 'Отложенное — отдельно от «Моих денег»')}
              style={{ flex: 'none', width: 20, height: 20, margin: 0, accentColor: 'var(--own)' }}
            />
          </label>
        )}
      </Cut>

      <CategoriesSection snap={snap} run={run} />

      <Cut id="fx" title="Курсы валют">
        <label className="setrow" style={{ cursor: 'pointer' }}>
          <span>
            <span>Курс ЦБ на дату операции</span>
            <span className="small muted">
              {snap.fx_auto
                ? (snap.fx_date ? `Загружены курсы на ${dMon(snap.fx_date)}. ` : 'Курсы загрузятся, когда появится интернет. ') + 'С сайта ЦБ запрашивается только дата.'
                : 'Выключено: валюта пересчитывается по курсам из настроек.'}
            </span>
          </span>
          <input
            type="checkbox"
            role="switch"
            aria-label="Курс ЦБ на дату операции"
            checked={snap.fx_auto}
            onChange={(e) => run(api.settingSet('fx.auto', e.target.checked ? '1' : '0'), e.target.checked ? 'Курс ЦБ на дату операции включён' : 'Курсы ЦБ выключены, считаю по курсам из настроек')}
            style={{ flex: 'none', width: 20, height: 20, margin: 0, accentColor: 'var(--own)' }}
          />
        </label>
        {snap.fx_auto &&
          snap.fx_date &&
          fxCurrencies
            .filter((c) => snap.fx[c])
            .map((c) => (
              <Row key={c} label={`1 ${c}${SIGN[c] ? ` (${SIGN[c]})` : ''}, ЦБ`}>
                <span className="num">{fxText(snap.fx[c])} ₽</span>
              </Row>
            ))}
        {snap.fx_auto && <FxRefresh run={run} flash={flash} />}
        <p className="small muted" style={{ margin: '10px 0 4px' }}>
          {snap.fx_auto ? 'Запасные курсы — если курс ЦБ не загрузился. Сколько рублей за единицу валюты:' : 'Сколько рублей за единицу валюты:'}
        </p>
        {fxCurrencies.map((c) => (
          <FxRow key={c} cur={c} value={snap.fx_manual[c]} save={(v) => run(api.settingSet('fx.' + c, v), `Курс ${c} сохранён`)} />
        ))}
      </Cut>

      <Reminders snap={snap} run={run} />

      <Wifi mobile={mobile} onSynced={refresh} />

      <CloudSync />

      <ClaudeSection mobile={mobile} run={run} flash={flash} locked={refresh} />

      <AiSection snap={snap} flash={flash} locked={refresh} />

      <BackupSection snap={snap} mobile={mobile} run={run} flash={flash} />

      <Cut id="look" title="Оформление" hint={`${SCHEMES[theme.scheme]?.name ?? ''}, ${theme.mode === 'dark' ? 'тёмная' : theme.mode === 'light' ? 'светлая' : 'как в системе'}`}>
        <div className="schemes" role="group" aria-label="Цветовая схема">
          {Object.entries(SCHEMES).map(([id, sc]) => {
            const t = dark ? sc.dark : sc.light;
            const on = id === theme.scheme;
            return (
              <button key={id} type="button" aria-pressed={on} className={'scheme' + (on ? ' on' : '')} onClick={() => setTheme({ ...theme, scheme: id })}>
                <span className="swatch" style={{ background: `linear-gradient(90deg, ${t.own} 50%, ${t.work} 50%)` }} />
                <span>{sc.name}</span>
              </button>
            );
          })}
        </div>
        <Seg
          cols
          small
          label="Тема"
          value={theme.mode}
          onChange={(mode) => setTheme({ ...theme, mode })}
          options={[
            ['light', 'Светлая'],
            ['dark', 'Тёмная'],
            ['system', 'Как в системе'],
          ]}
        />
      </Cut>

      <SecuritySection sec={security} os={info?.os ?? ''} />

      <Cut id="device" title="Это устройство">
        {info ? (
          <>
            <div className="row">
              <span className="muted">Устройство</span>
              <span className="row-val">{info.name}</span>
            </div>
            <div className="row">
              <span className="muted">Система</span>
              <span className="row-val">
                {info.os}, {info.arch}
              </span>
            </div>
            <div className="row">
              <span className="muted">Версия</span>
              <span className="row-val">{info.version}</span>
            </div>
            <div className="row">
              <span className="muted">База</span>
              <span className={info.encrypted ? 'row-val' : 'row-val warn-text'}>{info.encrypted ? `зашифрована, SQLCipher ${info.cipher}` : 'не зашифрована'}</span>
            </div>
            <div className="row">
              <span className="muted">Записей в журнале</span>
              <span className="row-val">{info.events.toLocaleString('ru-RU')}</span>
            </div>
            {info.unknown > 0 && (
              <div className="row">
                <span className="muted">Из более новой версии</span>
                <span className="row-val warn-text">{info.unknown.toLocaleString('ru-RU')} — видны после обновления «Кармана»</span>
              </div>
            )}
            <p className="small muted" style={{ overflowWrap: 'anywhere', marginTop: 6 }}>
              Папка данных: {info.data_dir}
            </p>
          </>
        ) : (
          <p className="small muted">Загружаю…</p>
        )}
      </Cut>

      <Cut id="checks" title="Проверки">
        <div className="stack gap16">
          <p className="small muted">Проверочная база: запись, чтение, шифрование файла. Расчёт: суммы из заявки 573 должны совпасть с 1С до копейки.</p>
          <CheckButton label="Проверить базу" run={api.dbCheck} />
          <CheckButton label="Проверить расчёт" run={api.calcCheck} />
        </div>
      </Cut>
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
        {error && <span className="small warn-text">Введите число больше нуля</span>}
      </span>
      <span style={{ display: 'flex', alignItems: 'center', gap: 6, flex: 'none' }}>
        <input
          className="input num"
          inputMode="decimal"
          aria-label={`Курс ${cur}, рублей`}
          value={text}
          placeholder="нет"
          onChange={(e) => setText(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => e.key === 'Enter' && (e.target as HTMLInputElement).blur()}
          style={{ width: 96, height: 40, textAlign: 'right', fontSize: 16 }}
        />
        <span className="small muted">₽</span>
      </span>
    </label>
  );
}
