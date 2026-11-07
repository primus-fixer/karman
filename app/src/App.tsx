import { useCallback, useEffect, useRef, useState } from 'react';
import { api, inApp, LOCKED, message, onLocked, type DeviceInfo, type LockStatus, type Op, type Snapshot, type TripStatus } from './api';
import Entry, { defaultDraft, draftFromOp, tripDraft, type EntryDraft } from './Entry';
import { rub } from './format';
import { bioLabel, LockScreen, PinFlow, type PinMode } from './Lock';
import Feed from './screens/Feed';
import Overview from './screens/Overview';
import Settings, { type ThemeState } from './screens/Settings';
import TripScreen, { type TripActions, type TripTab } from './screens/Trip';
import TripForm from './screens/TripForm';
import Trips from './screens/Trips';
import { applyTheme, type ThemeMode } from './theme';
import { Icon, type OpActions } from './ui';

export type Tab = 'overview' | 'feed' | 'trips' | 'settings';
type Screen = Tab | 'trip' | 'trip-form';

export type Nav = {
  go: (tab: Tab) => void;
  openTrip: (id: string) => void;
  newTrip: () => void;
  editTrip: (id: string) => void;
  openEntry: () => void;
};

const TABS: { id: Tab; label: string; icon: string }[] = [
  { id: 'overview', label: 'Обзор', icon: 'overview' },
  { id: 'feed', label: 'Лента', icon: 'feed' },
  { id: 'trips', label: 'Поездки', icon: 'trips' },
  { id: 'settings', label: 'Настройки', icon: 'settings' },
];

const ADVANCED: Partial<Record<TripStatus, string>> = {
  in_trip: 'Поездка началась',
  awaiting_report: 'Поездка завершена. Соберите документы для отчёта',
  review: 'Отчёт на проверке, ставка суточных зафиксирована',
  closed: 'Поездка закрыта',
};

function loadTheme(): ThemeState {
  try {
    const scheme = localStorage.getItem('karman.scheme') || 'shield';
    const mode = localStorage.getItem('karman.mode') as ThemeMode | null;
    return { scheme, mode: mode === 'dark' || mode === 'system' ? mode : 'light' };
  } catch {
    return { scheme: 'shield', mode: 'light' };
  }
}

function usePhone(): boolean {
  const q = () => window.innerWidth < 900;
  const [phone, setPhone] = useState(q);
  useEffect(() => {
    const on = () => setPhone(q());
    window.addEventListener('resize', on);
    return () => window.removeEventListener('resize', on);
  }, []);
  return phone;
}

async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
    return;
  } catch {
    /* ниже — запасной способ для WebView */
  }
  const ta = document.createElement('textarea');
  ta.value = text;
  ta.setAttribute('readonly', '');
  ta.style.position = 'fixed';
  ta.style.opacity = '0';
  document.body.appendChild(ta);
  ta.select();
  const ok = document.execCommand('copy');
  document.body.removeChild(ta);
  if (!ok) throw new Error('copy');
}

type Toast = { text: string; undo?: () => void; long?: boolean; key: number };

export default function App() {
  const isPhone = usePhone();
  const [snap, setSnap] = useState<Snapshot | null>(null);
  const [fatal, setFatal] = useState<string | null>(null);
  const [info, setInfo] = useState<DeviceInfo | null>(null);
  const [screen, setScreen] = useState<Screen>('overview');
  const [tripId, setTripId] = useState<string | null>(null);
  const [tripTab, setTripTab] = useState<TripTab>('summary');
  const [reportPart, setReportPart] = useState<'plan' | 'fact' | null>(null);
  const [tripFrom, setTripFrom] = useState<Tab>('trips');
  const [formId, setFormId] = useState<string | null>(null);
  const [entry, setEntry] = useState<{ draft: EntryDraft; key: number; editing: Op | null } | null>(null);
  const [toast, setToast] = useState<Toast | null>(null);
  const [theme, setTheme] = useState<ThemeState>(loadTheme);
  const [dark, setDark] = useState(false);
  const [lock, setLock] = useState<LockStatus | null>(null);
  const [locked, setLocked] = useState(false);
  const [lockKey, setLockKey] = useState(0);
  const [pinFlow, setPinFlow] = useState<PinMode | null>(null);
  const mainRef = useRef<HTMLElement>(null);
  const firstLoad = useRef(true);
  const notice = useRef<string | null>(null);

  // ——— Тема ———
  useEffect(() => {
    const apply = () => setDark(applyTheme(theme.scheme, theme.mode) === 'dark');
    apply();
    try {
      localStorage.setItem('karman.scheme', theme.scheme);
      localStorage.setItem('karman.mode', theme.mode);
    } catch {
      /* без сохранения темы */
    }
    if (theme.mode !== 'system') return;
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    mq.addEventListener('change', apply);
    return () => mq.removeEventListener('change', apply);
  }, [theme]);

  const flash = useCallback((text: string, undo?: () => void, long?: boolean) => setToast({ text, undo, long, key: Date.now() }), []);
  useEffect(() => {
    if (!toast) return;
    const id = window.setTimeout(() => setToast(null), toast.long ? 10000 : toast.undo ? 6000 : 3500);
    return () => window.clearTimeout(id);
  }, [toast]);

  // ——— Вход ———
  /** «Карман» заблокирован: показываем экран входа с актуальным состоянием. */
  const goLocked = useCallback(() => {
    api.lockStatus().then(
      (st) => {
        setLock(st);
        if (st.locked) {
          setLockKey((k) => k + 1);
          setLocked(true);
        }
      },
      () => setLocked(true),
    );
  }, []);

  // ——— Данные ———
  const refresh = useCallback(() => {
    api.snapshot().then(
      (s) => {
        setSnap(s);
        setFatal(null);
      },
      (e) => (message(e) === LOCKED ? goLocked() : setFatal(message(e))),
    );
    api.deviceInfo().then(setInfo, () => undefined);
  }, [goLocked]);
  const showNotice = useCallback(() => {
    if (notice.current) flash(notice.current, undefined, true);
    notice.current = null;
  }, [flash]);
  useEffect(() => {
    if (!inApp) {
      setFatal('Эта страница работает внутри приложения «Карман».');
      return;
    }
    api.lockStatus().then(
      (st) => {
        setLock(st);
        notice.current = st.notice;
        if (st.locked) {
          setLockKey((k) => k + 1);
          setLocked(true);
        } else {
          refresh();
          showNotice();
        }
      },
      (e) => setFatal(message(e)),
    );
    const onVisible = () => {
      const visible = document.visibilityState === 'visible';
      api.appVisible(visible).then(
        (st) => {
          setLock(st);
          if (st.locked) {
            if (visible) goLocked();
          } else if (visible) refresh();
        },
        () => undefined,
      );
    };
    document.addEventListener('visibilitychange', onVisible);
    const unlisten = onLocked(goLocked).catch(() => () => undefined);
    return () => {
      document.removeEventListener('visibilitychange', onVisible);
      unlisten.then((f) => f());
    };
  }, [refresh, goLocked, showNotice]);

  const onUnlocked = useCallback(
    (st: LockStatus) => {
      setLock(st);
      setLocked(false);
      refresh();
      showNotice();
      if (st.recovery) setPinFlow('reset');
    },
    [refresh, showNotice],
  );

  const run = useCallback(
    async (p: Promise<Snapshot>, ok?: string): Promise<Snapshot | null> => {
      try {
        const s = await p;
        setSnap(s);
        if (ok) flash(ok);
        return s;
      } catch (e) {
        if (message(e) === LOCKED) goLocked();
        else flash(message(e));
        return null;
      }
    },
    [flash, goLocked],
  );

  const security = {
    lock,
    onPin: (mode: PinMode) => setPinFlow(mode),
    onBio: async (on: boolean) => {
      try {
        const st = await api.bioSet(on);
        setLock(st);
        const name = bioLabel(st.bio_kind, info?.os ?? '').name;
        if (st.error) flash(st.error);
        else if (st.bio === on) flash(on ? `Вход: ${name} включён` : `Вход: ${name} выключен`);
      } catch (e) {
        if (message(e) === LOCKED) goLocked();
        else flash(message(e));
      }
    },
    onLockNow: async () => {
      const st = await api.lockNow();
      setLock(st);
      if (st.locked) {
        setLockKey((k) => k + 1);
        setLocked(true);
      }
    },
  };

  // ——— Кнопка «Назад» на Android: каждый шаг вглубь — запись в истории ———
  const depth = useRef(0);
  const skip = useRef(0);
  const backRef = useRef<() => void>(() => undefined);
  const pushHistory = () => {
    try {
      window.history.pushState({ karman: depth.current + 1 }, '');
      depth.current++;
    } catch {
      /* без истории */
    }
  };
  const clearHistory = () => {
    if (depth.current > 0) {
      skip.current++;
      const n = depth.current;
      depth.current = 0;
      window.history.go(-n);
    }
  };
  const back = () => {
    if (depth.current > 0) window.history.back();
    else backRef.current();
  };
  useEffect(() => {
    const onPop = () => {
      if (skip.current > 0) {
        skip.current--;
        return;
      }
      depth.current = Math.max(0, depth.current - 1);
      backRef.current();
    };
    window.addEventListener('popstate', onPop);
    return () => window.removeEventListener('popstate', onPop);
  }, []);
  backRef.current = () => {
    if (entry && isPhone) setEntry(null);
    else if (screen === 'trip-form') {
      if (formId) setScreen('trip');
      else setScreen(tripFrom);
    } else if (screen === 'trip') setScreen(tripFrom);
  };

  const toTop = () => mainRef.current?.scrollTo(0, 0);
  const currentTab = (): Tab => (screen === 'trip' || screen === 'trip-form' ? tripFrom : screen);

  const openEntry = (draft?: EntryDraft, editing?: Op) => {
    if (!snap) return;
    if (isPhone && !entry) pushHistory();
    if (isPhone) setToast(null);
    setEntry({ draft: draft ?? defaultDraft(snap), key: Date.now(), editing: editing ?? null });
  };
  const closeEntry = () => {
    if (isPhone) back();
    else setEntry(null);
  };

  const nav: Nav = {
    go: (tab) => {
      clearHistory();
      setScreen(tab);
      if (isPhone || (tab !== 'overview' && tab !== 'feed')) setEntry(null);
      toTop();
    },
    openTrip: (id) => {
      pushHistory();
      setTripFrom(currentTab());
      setTripId(id);
      setTripTab('summary');
      setReportPart(null);
      setScreen('trip');
      if (!isPhone) setEntry(null);
      toTop();
    },
    newTrip: () => {
      pushHistory();
      setTripFrom(currentTab());
      setFormId(null);
      setScreen('trip-form');
      if (!isPhone) setEntry(null);
      toTop();
    },
    editTrip: (id) => {
      pushHistory();
      setFormId(id);
      setScreen('trip-form');
      toTop();
    },
    openEntry: () => openEntry(),
  };

  // На широком экране панель ввода открыта сразу.
  useEffect(() => {
    if (snap && firstLoad.current) {
      firstLoad.current = false;
      if (!isPhone && window.innerWidth >= 1280) setEntry({ draft: defaultDraft(snap), key: Date.now(), editing: null });
    }
  }, [snap, isPhone]);

  const addDoc = (op: Op) => run(api.opSave({ ...op, doc: 'photo' }), 'Документ отмечен, расход вошёл в отчёт');
  const deleteOp = async (op: Op) => {
    const s = await run(api.opDelete(op.id));
    if (s) flash('Операция удалена', () => run(api.opSave(op), 'Операция возвращена'));
  };
  const opAct: OpActions = {
    addDoc: (op) => {
      addDoc(op);
    },
    remove: (op) => {
      deleteOp(op);
    },
    edit: (op) => openEntry(draftFromOp(op), op),
  };

  const tripAct: TripActions = {
    advance: async (id) => {
      const s = await run(api.tripAdvance(id));
      const tv = s?.trips.find((t) => t.trip.id === id);
      if (!tv) return;
      if (tv.trip.status === 'advance_received') flash('Аванс записан на рабочий счёт: ' + rub(tv.s.issued));
      else flash(ADVANCED[tv.trip.status] || 'Готово');
    },
    settle: async (id) => {
      const before = snap?.trips.find((t) => t.trip.id === id)?.s.balance ?? 0;
      await run(api.tripSettle(id), before < 0 ? 'Возмещение записано на личный счёт' : 'Возврат компании записан');
    },
    save: (trip) => {
      run(api.tripSave(trip), 'Сохранено');
    },
    remove: async (id) => {
      const s = await run(api.tripDelete(id), `Поездка ${id} удалена`);
      if (s) {
        clearHistory();
        setScreen('trips');
      }
    },
    addDoc: (op) => {
      addDoc(op);
    },
    toggleSent: (op) => {
      run(api.opSave({ ...op, sent: !op.sent }));
    },
    addOp: (id) => {
      const tv = snap?.trips.find((t) => t.trip.id === id);
      if (snap && tv) openEntry(tripDraft(snap, tv));
    },
    copy: (text, label) => {
      copyText(text).then(
        () => flash('Скопировано: ' + label),
        () => flash('Не удалось скопировать'),
      );
    },
  };

  const overlay =
    locked && lock ? (
      <LockScreen key={lockKey} status={lock} os={info?.os ?? ''} onUnlocked={onUnlocked} />
    ) : pinFlow ? (
      <PinFlow
        mode={pinFlow}
        onClose={() => setPinFlow(null)}
        onDone={(st, text) => {
          setLock(st);
          setPinFlow(null);
          flash(text);
        }}
      />
    ) : null;

  if (!snap) {
    return (
      <>
        <div className="app" inert={!!overlay}>
          <main className="main" style={{ display: 'grid', placeItems: 'center', padding: 24 }}>
            {fatal ? (
              <div className="stack gap12" style={{ maxWidth: 420 }}>
                <h1 className="h1">Карман</h1>
                <p className="error">{fatal}</p>
              </div>
            ) : (
              <p className="note">{locked ? '' : 'Открываю базу…'}</p>
            )}
          </main>
        </div>
        {overlay}
      </>
    );
  }

  const mobile = info ? info.os === 'android' || info.os === 'ios' : false;
  let body = null;
  if (screen === 'overview') body = <Overview snap={snap} nav={nav} act={opAct} />;
  else if (screen === 'feed') body = <Feed snap={snap} act={opAct} />;
  else if (screen === 'trips') body = <Trips snap={snap} nav={nav} />;
  else if (screen === 'settings') body = <Settings snap={snap} info={info} mobile={mobile} run={run} refresh={refresh} theme={theme} setTheme={setTheme} dark={dark} security={security} />;
  else if (screen === 'trip' && tripId)
    body = (
      <TripScreen
        snap={snap}
        tripId={tripId}
        nav={nav}
        act={tripAct}
        tab={tripTab}
        setTab={setTripTab}
        part={reportPart}
        setPart={setReportPart}
        isPhone={isPhone}
        onBack={back}
        opAct={opAct}
      />
    );
  else if (screen === 'trip-form')
    body = (
      <TripForm
        key={formId ?? 'new'}
        snap={snap}
        tripId={formId}
        onCancel={back}
        onSave={async (trip, isNew) => {
          const s = await run(api.tripSave(trip), isNew ? `Поездка ${trip.id} создана` : 'Поездка сохранена');
          if (!s) return false;
          if (isNew) {
            // Форма сменяется карточкой поездки на том же шаге истории.
            setTripId(trip.id);
            setTripTab('summary');
            setReportPart(null);
            setScreen('trip');
            toTop();
          } else back();
          return true;
        }}
      />
    );

  const tab = currentTab();
  const entryEl = entry && (
    <Entry key={entry.key} snap={snap} initial={entry.draft} editing={entry.editing} isPhone={isPhone} mobile={mobile} onSave={(op) => run(api.opSave(op))} onClose={closeEntry} flash={flash} />
  );

  return (
    <>
    <div className={'app ' + (isPhone ? 'phone' : 'desktop')} inert={!!overlay}>
      {!isPhone && (
        <aside className="side">
          <p className="logo">Карман</p>
          <button type="button" className="newop" aria-expanded={!!entry} onClick={() => (entry ? setEntry(null) : openEntry())}>
            <Icon name={entry ? 'x' : 'plus'} size={20} stroke={2} />
            Новая операция
          </button>
          {TABS.map((t) => (
            <button key={t.id} type="button" className={'navitem' + (tab === t.id ? ' on' : '')} aria-current={tab === t.id ? 'page' : undefined} onClick={() => nav.go(t.id)}>
              <Icon name={t.icon} size={20} />
              {t.label}
            </button>
          ))}
        </aside>
      )}
      <main className="main" ref={mainRef}>
        <div className={'inner' + (screen === 'trip' ? '' : ' narrow')}>{body}</div>
      </main>
      {!isPhone && entryEl}
      {isPhone && (
        <nav className="bottomnav" aria-label="Разделы">
          {TABS.map((t) => (
            <button key={t.id} type="button" className={tab === t.id ? 'on' : ''} aria-current={tab === t.id ? 'page' : undefined} onClick={() => nav.go(t.id)}>
              <Icon name={t.icon} size={22} stroke={tab === t.id ? 2 : 1.75} />
              {t.label}
            </button>
          ))}
        </nav>
      )}
      {isPhone && !entry && (screen === 'overview' || screen === 'feed' || screen === 'trips') && (
        <button type="button" className="fab" aria-label="Записать операцию" onClick={() => openEntry()}>
          <Icon name="plus" size={26} stroke={2} />
        </button>
      )}
      {isPhone && entry && (
        <>
          <button type="button" className="scrim" aria-label="Закрыть ввод" onClick={closeEntry} />
          {entryEl}
        </>
      )}
      {toast && (
        <div key={toast.key} className="toast" role="status">
          <span>{toast.text}</span>
          {toast.undo && (
            <button
              type="button"
              onClick={() => {
                toast.undo?.();
                setToast(null);
              }}
            >
              Вернуть
            </button>
          )}
        </div>
      )}
    </div>
    {overlay}
    </>
  );
}
