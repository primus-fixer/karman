import { useEffect, useState } from 'react';
import { api, type AiFeature, type AiStatus } from './api';

// Состояние ИИ на этом устройстве — одно на все экраны: раздел «ИИ» в настройках его обновляет.
let current: AiStatus | null = null;
let loading = false;
const subs = new Set<(s: AiStatus | null) => void>();

export function setAi(s: AiStatus | null) {
  current = s;
  subs.forEach((f) => f(s));
}

/** Загрузить, если ещё не загружено (пока «Карман» заблокирован, ответа нет — попробуем позже). */
export function loadAi(force = false) {
  if (loading || (current && !force)) return;
  loading = true;
  api.aiStatus().then(
    (s) => {
      loading = false;
      setAi(s);
    },
    () => {
      loading = false;
    },
  );
}

export function useAi(): AiStatus | null {
  const [s, setS] = useState(current);
  useEffect(() => {
    subs.add(setS);
    loadAi();
    return () => {
      subs.delete(setS);
    };
  }, []);
  return s;
}

/** Функция доступна: ключ есть и она включена. */
export const aiOn = (s: AiStatus | null, f: AiFeature) => !!s && s.key && !!s.features?.[f];
