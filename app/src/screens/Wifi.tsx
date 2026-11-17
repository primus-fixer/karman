// Связь по Wi-Fi — общий раздел из kit; здесь — слова «Кармана».
import { WifiSection as Section, type WifiWords } from '@primus-fixer/kit/ui';
import { api, LOCKED, message, scanQr } from '../api';
import { Cut } from '../ui';

export { agoText } from '@primus-fixer/kit/ui';

const WORDS: WifiWords = { name: 'Карман', feminine: false, files: 'снимков чеков', filesNote: 'Снимки чеков едут вместе с записями.', where: 'Настройки → Обмен по Wi-Fi', port: 47500 };

/** Настройки → «Обмен по Wi-Fi»: связанные устройства, код связи, кто рядом. Раздел — под катом «Кармана». */
export default function Wifi({ mobile, onSynced }: { mobile: boolean; onSynced: () => void }) {
  return (
    <Section
      api={api}
      words={WORDS}
      mobile={mobile}
      scanQr={scanQr}
      message={message}
      locked={LOCKED}
      onSynced={onSynced}
      wrap={(hint, children) => (
        <Cut id="wifi" title="Обмен по Wi-Fi" hint={hint}>
          {children}
        </Cut>
      )}
    />
  );
}
