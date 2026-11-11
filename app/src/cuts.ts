import { useEffect, useState } from 'react';

// Разделы настроек под катом: какие раскрыты — помнит этот браузер (у каждого устройства своё).
const KEY = 'karman.open';
let open: Set<string> | null = null;
const subs = new Set<() => void>();

function load(): Set<string> {
  if (!open) {
    try {
      const v: unknown = JSON.parse(localStorage.getItem(KEY) || '[]');
      open = new Set(Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : []);
    } catch {
      open = new Set();
    }
  }
  return open;
}

export function cutOpen(id: string): boolean {
  return load().has(id);
}

/** Раскрыть или свернуть раздел (и запомнить). */
export function setCut(id: string, on: boolean) {
  const s = load();
  if (on) s.add(id);
  else s.delete(id);
  try {
    localStorage.setItem(KEY, JSON.stringify([...s]));
  } catch {
    /* хранилище недоступно — помним до перезапуска */
  }
  subs.forEach((f) => f());
}

export function useCut(id: string): [boolean, (on: boolean) => void] {
  const [, tick] = useState(0);
  useEffect(() => {
    const f = () => tick((n) => n + 1);
    subs.add(f);
    return () => {
      subs.delete(f);
    };
  }, []);
  return [cutOpen(id), (on: boolean) => setCut(id, on)];
}

/**
 * Открыть раздел настроек (после перехода на экран настроек): раскрыть и полторы секунды держать
 * его вверху — разделы выше догружаются и становятся выше, — пока человек сам не прокрутит.
 */
export function showSection(id: string) {
  setCut(id, true);
  let stop = false;
  let want: number | null = null;
  const cancel = () => (stop = true);
  const events = ['wheel', 'touchstart', 'keydown', 'mousedown'] as const;
  events.forEach((e) => window.addEventListener(e, cancel, { passive: true }));
  const t0 = Date.now();
  const tick = () => {
    const el = document.getElementById(id);
    if (el && !stop) {
      const top = el.getBoundingClientRect().top;
      if (want === null || Math.abs(top - want) > 2) {
        el.scrollIntoView({ block: 'start' });
        want = el.getBoundingClientRect().top;
      }
    }
    if (!stop && Date.now() - t0 < 1500) window.requestAnimationFrame(tick);
    else events.forEach((e) => window.removeEventListener(e, cancel));
  };
  window.setTimeout(tick, 80);
}
