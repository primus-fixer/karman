import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { api, inApp, LOCKED, message, onBackup, onClaude, onCloud, onFx, onLan, onLocked, type Attachment, type DeviceInfo, type LockStatus, type Op, type Snapshot, type TripStatus } from './api';
import Entry, { defaultDraft, draftFromOp, tripDraft, type EntryDraft } from './Entry';
import { aiOn, loadAi, useAi } from './aiState';
import ChatSheet from './screens/Chat';
import { rub } from './format';
import { applyCats, dueText } from './logic';
import { applyTheme, bioLabel, LockScreen, PinFlow, store, stored, type LockNames, type PinMode, type ThemeMode } from '@primus-fixer/kit/ui';
import { attachFile, pickFiles } from './media';
import Feed from './screens/Feed';
import Finance, { type FinanceActions, type FinView } from './screens/Finance';
import Overview from './screens/Overview';
import Settings, { type ThemeState } from './screens/Settings';
import type { OtActions } from './screens/Overtime';
import TripScreen, { type TripActions, type TripTab } from './screens/Trip';
import TripForm from './screens/TripForm';
import Trips from './screens/Trips';
import { Icon, Viewer, type OpActions } from './ui';
import { showSection } from './cuts';

export type Tab = 'overview' | 'feed' | 'trips' | 'finance' | 'settings';
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
  { id: 'finance', label: 'Финансы', icon: 'finance' },
  { id: 'settings', label: 'Настройки', icon: 'settings' },
];

const ADVANCED: Partial<Record<TripStatus, string>> = {
  in_trip: 'Поездка началась',
  awaiting_report: 'Поездка завершена. Соберите документы для отчёта',
  review: 'Отчёт на проверке, ставка суточных зафиксирована',
  closed: 'Поездка закрыта',
};

/** Название на экране входа: «Карман», «Вход в „Карман“». */
const LOCK_NAMES: LockNames = { name: 'Карман', nameAcc: 'Карман' };

function loadTheme(): ThemeState {
  const scheme = stored('karman.scheme', 'shield');
  const mode = stored('karman.mode', 'light') as ThemeMode;
  return { scheme, mode: mode === 'dark' || mode === 'system' ? mode : 'light' };
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

/** Сообщение внизу; `undo` — кнопка (по умолчанию «Вернуть», иначе `label`). */
type Toast = { text: string; undo?: () => void; long?: boolean; label?: string; key: number };

export default function App() {
  const isPhone = usePhone();
  const [snap, setSnap] = useState<Snapshot | null>(null);
  // Свои категории и скрытые — в справочник категорий до отрисовки экранов.
  useMemo(() => snap && applyCats(snap), [snap]);
  const [fatal, setFatal] = useState<string | null>(null);
  const [info, setInfo] = useState<DeviceInfo | null>(null);
  const [screen, setScreen] = useState<Screen>('overview');
  const [tripId, setTripId] = useState<string | null>(null);
  const [tripTab, setTripTab] = useState<TripTab>('summary');
  const [reportPart, setReportPart] = useState<'plan' | 'fact' | null>(null);
  const [tripFrom, setTripFrom] = useState<Tab>('trips');
  const [formId, setFormId] = useState<string | null>(null);
  const [entry, setEntry] = useState<{ draft: EntryDraft; key: number; editing: Op | null } | null>(null);
  /** «Спросить Claude» открыт (на компьютере — вместо панели ввода, на телефоне — поверх экрана). */
  const [chat, setChat] = useState(false);
  const ai = useAi();
  const canChat = aiOn(ai, 'chat');
  const [toast, setToast] = useState<Toast | null>(null);
  const [theme, setTheme] = useState<ThemeState>(loadTheme);
  const [dark, setDark] = useState(false);
  const [lock, setLock] = useState<LockStatus | null>(null);
  const [locked, setLocked] = useState(false);
  // Экран входа уже открыт — заново его не создаём. Иначе после отмены отпечатка на телефоне
  // (окно отпечатка уводит «Карман» в фон и обратно) он снова и снова сам спрашивал отпечаток.
  const lockedRef = useRef(false);
  const [lockKey, setLockKey] = useState(0);
  const [pinFlow, setPinFlow] = useState<PinMode | null>(null);
  const [viewer, setViewer] = useState<Attachment | null>(null);
  const [finView, setFinView] = useState<FinView>('pay');
  const mainRef = useRef<HTMLElement>(null);
  const firstLoad = useRef(true);
  const notice = useRef<string | null>(null);

  // ——— Тема ———
  useEffect(() => {
    const apply = () => setDark(applyTheme(theme.scheme, theme.mode) === 'dark');
    apply();
    store('karman.scheme', theme.scheme);
    store('karman.mode', theme.mode);
    if (theme.mode !== 'system') return;
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    mq.addEventListener('change', apply);
    return () => mq.removeEventListener('change', apply);
  }, [theme]);

  const flash = useCallback((text: string, undo?: () => void, long?: boolean, label?: string) => setToast({ text, undo, long, label, key: Date.now() }), []);
  useEffect(() => {
    if (!toast) return;
    const id = window.setTimeout(() => setToast(null), toast.long ? 10000 : toast.undo ? 6000 : 3500);
    return () => window.clearTimeout(id);
  }, [toast]);

  // ——— Вход ———
  /** Новый экран входа (он сам спросит отпечаток) — только если его ещё нет на экране. */
  const showLock = useCallback(() => {
    if (lockedRef.current) return;
    lockedRef.current = true;
    setLockKey((k) => k + 1);
    setLocked(true);
  }, []);
  /** «Карман» заблокирован: показываем экран входа с актуальным состоянием. */
  const goLocked = useCallback(() => {
    api.lockStatus().then(
      (st) => {
        setLock(st);
        if (st.locked) showLock();
      },
      () => showLock(),
    );
  }, [showLock]);

  // ——— Данные ———
  const refresh = useCallback(() => {
    api.snapshot().then(
      (s) => {
        setSnap(s);
        setFatal(null);
        // Состояние ИИ — после входа (пока «Карман» заблокирован, его не прочитать).
        loadAi();
        // На широком экране панель ввода открыта сразу — в том же обновлении, что и первые данные.
        // Отдельный эффект после отрисовки спорил с первым быстрым нажатием, и панель оставалась на экране.
        if (firstLoad.current) {
          firstLoad.current = false;
          if (window.innerWidth >= 1280) setEntry({ draft: defaultDraft(s), key: Date.now(), editing: null });
        }
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
        notice.current = st.notice ?? null;
        if (st.locked) {
          showLock();
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
    // Свежие курсы ЦБ пересчитали остатки на счетах в валюте. Пока «Карман» заблокирован, данные не придут — и не нужны.
    const unFx = onFx(() => api.snapshot().then(setSnap, () => undefined)).catch(() => () => undefined);
    // Автокопия записала, когда сделана, — обновить напоминание о копиях.
    const unBackup = onBackup(() => api.snapshot().then(setSnap, () => undefined)).catch(() => () => undefined);
    // Через облако пришли записи или снимки чеков — обновить экраны.
    const unCloud = onCloud((e) => {
      if (e.received > 0) api.snapshot().then(setSnap, () => undefined);
    }).catch(() => () => undefined);
    // Обмен по Wi-Fi со связанным устройством принёс записи — свежая сводка.
    const unLan = onLan((e) => {
      if (e.received > 0) api.snapshot().then(setSnap, () => undefined);
    }).catch(() => () => undefined);
    return () => {
      document.removeEventListener('visibilitychange', onVisible);
      unlisten.then((f) => f());
      unFx.then((f) => f());
      unBackup.then((f) => f());
      unCloud.then((f) => f());
      unLan.then((f) => f());
    };
  }, [refresh, goLocked, showNotice, showLock]);

  const onUnlocked = useCallback(
    (st: LockStatus) => {
      setLock(st);
      lockedRef.current = false;
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

  // Claude записал что-то через коннектор: свежие данные и сообщение с «Вернуть».
  useEffect(() => {
    const un = onClaude((e) => {
      api.snapshot().then(setSnap, () => undefined);
      flash(e.text, e.id ? () => run(api.claudeUndo(e.id), 'Отменено') : undefined, true);
    }).catch(() => () => undefined);
    return () => {
      un.then((f) => f());
    };
  }, [flash, run]);

  /** Первый платёж или долг со сроком: на Android попросить разрешение на уведомления. */
  const ensureNotify = useCallback(() => {
    api.remindStatus(false).then(
      (st) => {
        if (st.platform !== 'android' || st.permission !== 'prompt') return;
        api.remindStatus(true).then(
          (after) => {
            if (after.permission === 'granted') refresh();
            else flash('Уведомления выключены: напоминания будут только в приложении. Включить — в настройках телефона', undefined, true);
          },
          () => undefined,
        );
      },
      () => undefined,
    );
  }, [flash, refresh]);
  const finAct: FinanceActions = { run, saved: ensureNotify, flash: (text, long) => flash(text, undefined, long), locked: goLocked };

  // Напоминание при открытии: что пора оплатить и какие сроки близко (один раз за запуск).
  const reminded = useRef(false);
  useEffect(() => {
    if (!snap || reminded.current) return;
    reminded.current = true;
    const soon = snap.upcoming.filter((u) => u.soon);
    if (!soon.length) return;
    const first = soon[0];
    const what = first.kind === 'plan' ? (first.auto ? 'автосписание' : 'платёж') : first.kind === 'deposit' ? 'заканчивается вклад' : 'срок долга';
    let one: string;
    if (first.days < 0) one = (first.kind === 'plan' && first.auto ? 'Просрочено автосписание' : first.kind === 'plan' ? 'Просрочен платёж' : first.kind === 'deposit' ? 'Закончился вклад' : 'Прошёл срок долга') + ': ' + first.title;
    else {
      const when = dueText(first.days);
      one = `${when[0].toUpperCase() + when.slice(1)} ${what}: ${first.title}`;
    }
    const text = soon.length === 1 ? one : `Скоро платежи и сроки: ${soon.length}. Ближайший — ${first.title}, ${dueText(first.days)}`;
    flash(text, () => {
      clearHistory();
      setFinView('pay');
      setScreen('finance');
      setEntry((e) => (isPhone ? null : e));
    }, true, 'Открыть');
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [snap]);

  const security = {
    lock,
    onPin: (mode: PinMode) => setPinFlow(mode),
    onBio: async (on: boolean) => {
      try {
        const st = await api.bioSet(on);
        setLock(st);
        const name = bioLabel(st.bio_kind).name;
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
      if (st.locked) showLock();
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
    if (chat && isPhone) setChat(false);
    else if (entry && isPhone) setEntry(null);
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
    setChat(false);
    setEntry({ draft: draft ?? defaultDraft(snap), key: Date.now(), editing: editing ?? null });
  };
  /** «Спросить Claude»: на компьютере — вместо панели ввода. */
  const openChat = () => {
    if (isPhone && !chat) pushHistory();
    if (!isPhone) setEntry(null);
    setChat(true);
  };
  const closeChat = () => {
    if (isPhone) back();
    else setChat(false);
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

  const addDoc = (op: Op) => run(api.opSave({ ...op, doc: 'photo' }), 'Документ отмечен, расход вошёл в отчёт');
  const deleteOp = async (op: Op) => {
    const s = await run(api.opDelete(op.id));
    if (s) flash('Операция удалена', () => run(api.opSave(op), 'Операция возвращена'));
  };
  const mobileNow = info ? info.os === 'android' || info.os === 'ios' : false;
  /** Снимок чека к записанной операции: на телефоне — камера, на компьютере — файлы. */
  const attachTo = async (op: Op) => {
    const files = await pickFiles({ accept: mobileNow ? 'image/*' : 'image/*,application/pdf', capture: mobileNow, multiple: !mobileNow });
    if (!files.length) return;
    try {
      const added: Attachment[] = [];
      for (const f of files) added.push(await attachFile(f));
      const onlyPdf = added.every((a) => a.mime === 'application/pdf');
      const doc = op.kind === 'work' && !op.doc ? (onlyPdf ? 'file' : 'photo') : op.doc;
      await run(api.opSave({ ...op, att: [...(op.att ?? []), ...added], doc }), added.length > 1 ? 'Снимки прикреплены' : 'Снимок прикреплён');
    } catch (e) {
      if (message(e) === LOCKED) goLocked();
      else flash(message(e));
    }
  };
  const opAct: OpActions = {
    addDoc: (op) => {
      addDoc(op);
    },
    remove: (op) => {
      deleteOp(op);
    },
    edit: (op) => openEntry(draftFromOp(op), op),
    attach: (op) => {
      attachTo(op);
    },
    view: (a) => setViewer(a),
  };

  const tripAct: TripActions = {
    locked: goLocked,
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
    zip: (id) => {
      api.receiptsZip(id).then(
        (path) => flash('Архив чеков сохранён: ' + path, undefined, true),
        (e) => (message(e) === LOCKED ? goLocked() : flash(message(e))),
      );
    },
  };

  const otAct: OtActions = { run, flash: (text, undo, long) => flash(text, undo, long), locked: goLocked, view: (a) => setViewer(a), mobile: mobileNow };

  const overlay =
    locked && lock ? (
      <LockScreen key={lockKey} status={lock} api={api} names={LOCK_NAMES} onUnlocked={onUnlocked} />
    ) : pinFlow ? (
      <PinFlow
        mode={pinFlow}
        api={api}
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
                <h1 className="screen-title">Карман</h1>
                <p className="error">{fatal}</p>
              </div>
            ) : (
              <p className="small muted">{locked ? '' : 'Открываю базу…'}</p>
            )}
          </main>
        </div>
        {overlay}
      </>
    );
  }

  const mobile = info ? info.os === 'android' || info.os === 'ios' : false;
  let body = null;
  if (screen === 'overview')
    body = (
      <Overview
        snap={snap}
        nav={nav}
        act={opAct}
        fin={finAct}
        mobile={mobile}
        ask={canChat ? openChat : undefined}
        openFinance={(v) => {
          setFinView(v);
          nav.go('finance');
        }}
      />
    );
  else if (screen === 'feed') body = <Feed snap={snap} act={opAct} />;
  else if (screen === 'trips') body = <Trips snap={snap} nav={nav} />;
  else if (screen === 'finance')
    body = (
      <Finance
        snap={snap}
        act={finAct}
        view={finView}
        setView={setFinView}
        goSettings={() => {
          nav.go('settings');
          showSection('accounts');
        }}
        openTrip={nav.openTrip}
        mobile={mobile}
      />
    );
  else if (screen === 'settings')
    body = <Settings snap={snap} info={info} mobile={mobile} run={run} refresh={refresh} theme={theme} setTheme={setTheme} dark={dark} security={security} flash={(text, long) => flash(text, undefined, long)} />;
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
        ot={otAct}
      />
    );
  else if (screen === 'trip-form')
    body = (
      <TripForm
        key={formId ?? 'new'}
        snap={snap}
        tripId={formId}
        mobile={mobile}
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
  const showFab = isPhone && !entry && !chat && (screen === 'overview' || screen === 'feed' || screen === 'trips');
  const entryEl = entry && (
    <Entry key={entry.key} snap={snap} initial={entry.draft} editing={entry.editing} isPhone={isPhone} mobile={mobile} onSave={(op) => run(api.opSave(op))} onClose={closeEntry} flash={flash} />
  );
  const chatEl = chat && canChat && <ChatSheet snap={snap} isPhone={isPhone} onClose={closeChat} locked={goLocked} />;

  return (
    <>
    <div className={isPhone ? 'app phone' : 'app wide'} inert={!!overlay}>
      {!isPhone && (
        <aside className="side">
          <p className="wordmark num">Карман</p>
          <button type="button" className="btn primary side-add" aria-expanded={!!entry} onClick={() => (entry ? setEntry(null) : openEntry())}>
            <Icon name={entry ? 'x' : 'plus'} size={20} stroke={2} />
            Новая операция
          </button>
          {canChat && (
            <button type="button" className={'navitem' + (chat ? ' on' : '')} aria-expanded={chat} onClick={() => (chat ? setChat(false) : openChat())}>
              <Icon name="spark" size={20} />
              Спросить Claude
            </button>
          )}
          {TABS.map((t) => (
            <button key={t.id} type="button" className={'navitem' + (tab === t.id ? ' on' : '')} aria-current={tab === t.id ? 'page' : undefined} onClick={() => nav.go(t.id)}>
              <Icon name={t.icon} size={20} />
              {t.label}
            </button>
          ))}
        </aside>
      )}
      <main className="main" ref={mainRef}>
        <div className={'inner' + (screen === 'trip' ? '' : ' narrow') + (showFab ? ' with-fab' : '')}>{body}</div>
      </main>
      {!isPhone && (chatEl || entryEl)}
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
      {isPhone && chatEl && (
        <>
          <button type="button" className="scrim" aria-label="Закрыть «Спросить Claude»" onClick={closeChat} />
          {chatEl}
        </>
      )}
      {showFab && (
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
        <div key={toast.key} className={toast.undo ? 'toast with-action' : 'toast'} role="status">
          <span>{toast.text}</span>
          {toast.undo && (
            <button
              type="button"
              className="toast-btn"
              onClick={() => {
                toast.undo?.();
                setToast(null);
              }}
            >
              {toast.label ?? 'Вернуть'}
            </button>
          )}
        </div>
      )}
    </div>
    {viewer && <Viewer att={viewer} pdfInline={!mobileNow} onClose={() => setViewer(null)} />}
    {overlay}
    </>
  );
}
