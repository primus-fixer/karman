// Облако — общий раздел из kit; здесь — слова «Кармана». Вход по коду (Яндекс, Google) — только у «Кармана».
import { CloudSection as Section, type CloudWords } from '@primus-fixer/kit/ui';
import { api, LOCKED, message } from '../api';
import { Cut } from '../ui';

export { lastText, stepText } from '@primus-fixer/kit/ui';

export const CLOUD_WORDS: CloudWords = {
  name: 'Карман',
  nameGen: 'Кармана',
  feminine: false,
  what: 'Записи и снимки чеков',
  whyNote: '— в командировке общий Wi-Fi не нужен',
  blobs: { many: 'снимков чеков', upload: 'Отправляю снимки чеков', download: 'Получаю снимки чеков', waiting: ['снимок чека придёт', 'снимка чеков придут', 'снимков чеков придут'] },
};

/** Настройки → «Облако»: Яндекс Диск или свой сервер WebDAV по паролю, Яндекс или Google — по коду. */
export default function CloudSync() {
  return (
    <Section
      api={api}
      words={CLOUD_WORDS}
      message={message}
      locked={LOCKED}
      wrap={(hint, children) => (
        <Cut id="cloud" title="Облако" hint={hint}>
          {children}
        </Cut>
      )}
    />
  );
}
