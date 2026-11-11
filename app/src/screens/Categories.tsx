import { useState } from 'react';
import { api, LOCKED, message, type Category, type Snapshot } from '../api';
import { CATS, CAT_ICONS, OWN_OUT_ALL, hiddenKey, isCustomCat, isHiddenCat } from '../logic';
import { Cut, ErrorText, Icon } from '../ui';

type Run = (p: Promise<Snapshot>, ok?: string) => Promise<Snapshot | null>;

/** Подписи иконок для экранного диктора. */
const ICON_LABEL: Record<string, string> = {
  groceries: 'Тележка',
  cafe: 'Чашка',
  food: 'Вилка и нож',
  transport: 'Автобус',
  auto: 'Автомобиль',
  taxi: 'Такси',
  car: 'Заправка',
  tickets: 'Поезд',
  lodging: 'Кровать',
  home: 'Дом',
  telecom: 'Телефон',
  subscriptions: 'Повтор',
  health: 'Сердце',
  clothes: 'Футболка',
  beauty: 'Ножницы',
  kids: 'Коляска',
  pets: 'Лапа',
  sport: 'Гантель',
  fun: 'Хлопушка',
  music: 'Ноты',
  gamepad: 'Джойстик',
  education: 'Шапка выпускника',
  book: 'Книга',
  travel: 'Зонт от солнца',
  gifts: 'Подарок',
  star: 'Звезда',
  sprout: 'Росток',
  bolt: 'Молния',
  tools: 'Гаечный ключ',
  taxes: 'Здание с колоннами',
  credit: 'Карта',
  salary: 'Купюра',
  debt: 'Люди',
  other: 'Три точки',
};

/** Своя категория: название и иконка из набора. */
function CategoryEditor({ initial, onSave, onCancel }: { initial: Category; onSave: (c: Category) => Promise<string | null>; onCancel: () => void }) {
  const [name, setName] = useState(initial.name);
  const [icon, setIcon] = useState(initial.icon || 'star');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const save = async () => {
    setBusy(true);
    setError(await onSave({ ...initial, name, icon }));
    setBusy(false);
  };
  return (
    <form
      className="card stack gap10 cated"
      onSubmit={(e) => {
        e.preventDefault();
        save();
      }}
    >
      <label className="label">
        <span>Название</span>
        <input className="field" value={name} maxLength={24} autoFocus onChange={(e) => setName(e.target.value)} placeholder="Например, Дача" aria-label="Название категории" />
      </label>
      <span className="note">Иконка</span>
      <div className="iconpick" role="radiogroup" aria-label="Иконка категории">
        {CAT_ICONS.map((i) => (
          <button key={i} type="button" role="radio" aria-checked={i === icon} aria-label={ICON_LABEL[i] ?? i} title={ICON_LABEL[i]} className={'iconbtn' + (i === icon ? ' sel' : '')} onClick={() => setIcon(i)}>
            <Icon name={i} size={22} />
          </button>
        ))}
      </div>
      <ErrorText text={error} />
      <div className="wrap">
        <button type="submit" className="btn own" disabled={busy || !name.trim()}>
          {busy ? 'Сохраняю…' : initial.id ? 'Сохранить' : 'Добавить'}
        </button>
        <button type="button" className="btn ghost" onClick={onCancel} disabled={busy}>
          Отмена
        </button>
      </div>
    </form>
  );
}

/** Настройки → «Категории трат»: встроенные и свои категории личных трат, скрыть и вернуть. */
export default function CategoriesSection({ snap, run }: { snap: Snapshot; run: Run }) {
  // Какая категория правится: её код или «new» — новая.
  const [edit, setEdit] = useState<string | null>(null);
  const toggle = (id: string, show: boolean) => {
    const name = (CATS[id] || CATS.other).n;
    run(api.settingSet(hiddenKey(id), show ? '0' : '1'), show ? `«${name}» — снова при вводе` : `«${name}» скрыта: при вводе её не будет`);
  };
  const save = async (c: Category): Promise<string | null> => {
    try {
      const s = await api.categorySave(c);
      await run(Promise.resolve(s), c.id ? 'Категория сохранена' : `Категория «${c.name.trim().replace(/\s+/g, ' ')}» добавлена`);
      setEdit(null);
      return null;
    } catch (e) {
      // Заблокировано — к экрану входа, как везде; остальное — под формой.
      if (message(e) === LOCKED) {
        run(Promise.reject(e));
        return null;
      }
      return message(e);
    }
  };
  const custom = (snap.categories ?? []).length;
  return (
    <Cut id="cats" title="Категории трат">
      <p className="note" style={{ marginBottom: 8 }}>
        Категории личных и семейных трат. Скрытую не предлагает окно ввода, а записанные операции, лимиты и регулярные платежи остаются с ней. Свои категории видны на всех ваших устройствах.
      </p>
      {OWN_OUT_ALL.map((id) => {
        const c = CATS[id] || CATS.other;
        const own = isCustomCat(id);
        const hidden = isHiddenCat(snap.settings, id);
        if (edit === id) {
          const cat = snap.categories?.find((x) => x.id === id);
          return cat ? <CategoryEditor key={id} initial={cat} onSave={save} onCancel={() => setEdit(null)} /> : null;
        }
        const note = [own ? 'своя' : '', hidden ? 'скрыта' : ''].filter(Boolean).join(', ');
        return (
          <div key={id} className={'setrow catrow' + (hidden ? ' off' : '')} style={{ cursor: 'default', minHeight: 54 }}>
            <span style={{ flexDirection: 'row', alignItems: 'center', gap: 12 }}>
              <Icon name={c.i} size={20} />
              <span className="stack" style={{ minWidth: 0 }}>
                <span>{c.n}</span>
                {note && <span className="note">{note.charAt(0).toUpperCase() + note.slice(1)}</span>}
              </span>
            </span>
            <span style={{ display: 'flex', alignItems: 'center', gap: 10, flex: 'none' }}>
              {own && (
                <button type="button" className="btn ghost small" onClick={() => setEdit(id)} aria-label={`Изменить «${c.n}»`}>
                  Изменить
                </button>
              )}
              {id === 'other' ? (
                <span className="note">всегда</span>
              ) : (
                <input
                  type="checkbox"
                  role="switch"
                  aria-label={`Показывать «${c.n}» при вводе`}
                  checked={!hidden}
                  onChange={(e) => toggle(id, e.target.checked)}
                  style={{ flex: 'none', width: 20, height: 20, margin: 0, accentColor: 'var(--own)' }}
                />
              )}
            </span>
          </div>
        );
      })}
      {edit === 'new' ? (
        <CategoryEditor initial={{ id: '', name: '', icon: 'star' }} onSave={save} onCancel={() => setEdit(null)} />
      ) : (
        <button type="button" className="btn ghost" style={{ marginTop: 10, alignSelf: 'flex-start' }} onClick={() => setEdit('new')} disabled={custom >= 50}>
          Добавить категорию
        </button>
      )}
    </Cut>
  );
}
