// Снимки и файлы чеков: выбор, сжатие и сохранение в зашифрованной базе.

import { api, type Attachment } from './api';

/** Снимок уменьшается до 1600 точек по длинной стороне — чек читается, а файл весит 200–400 КБ. */
const MAX_SIDE = 1600;
const PDF_LIMIT = 15 * 1024 * 1024;

/** Окно выбора файла. На телефоне с `capture` сразу открывается камера. */
export function pickFiles(opts: { accept: string; capture?: boolean; multiple?: boolean }): Promise<File[]> {
  return new Promise((resolve) => {
    const input = document.createElement('input');
    input.type = 'file';
    input.accept = opts.accept;
    if (opts.capture) input.setAttribute('capture', 'environment');
    if (opts.multiple) input.multiple = true;
    input.style.display = 'none';
    let done = false;
    const finish = (files: File[]) => {
      if (done) return;
      done = true;
      input.remove();
      resolve(files);
    };
    input.addEventListener('change', () => finish(Array.from(input.files || [])));
    // Отмена выбора. Если WebView её не присылает, обещание просто остаётся без ответа —
    // на экране это ничего не держит.
    input.addEventListener('cancel', () => finish([]));
    document.body.appendChild(input);
    input.click();
  });
}

function readDataUrl(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(String(r.result));
    r.onerror = () => reject(new Error('Файл не прочитался'));
    r.readAsDataURL(blob);
  });
}

const base64Of = (dataUrl: string) => dataUrl.slice(dataUrl.indexOf(',') + 1);

function loadImage(src: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve(img);
    img.onerror = () => reject(new Error('Снимок не открылся: подходят JPEG, PNG и WebP'));
    img.src = src;
  });
}

/** Уменьшает снимок и сохраняет в JPEG. */
async function toJpeg(file: File): Promise<string> {
  const img = await loadImage(await readDataUrl(file));
  const scale = Math.min(1, MAX_SIDE / Math.max(img.naturalWidth, img.naturalHeight));
  const w = Math.max(1, Math.round(img.naturalWidth * scale));
  const h = Math.max(1, Math.round(img.naturalHeight * scale));
  const canvas = document.createElement('canvas');
  canvas.width = w;
  canvas.height = h;
  const ctx = canvas.getContext('2d');
  if (!ctx) throw new Error('Снимок не обработался');
  ctx.fillStyle = '#fff';
  ctx.fillRect(0, 0, w, h);
  ctx.drawImage(img, 0, 0, w, h);
  const blob = await new Promise<Blob>((resolve, reject) => canvas.toBlob((b) => (b ? resolve(b) : reject(new Error('Снимок не сохранился'))), 'image/jpeg', 0.82));
  return base64Of(await readDataUrl(blob));
}

const isPdf = (f: File) => f.type === 'application/pdf' || f.name.toLowerCase().endsWith('.pdf');

/** Сохраняет файл в базе и возвращает вложение для операции. */
export async function attachFile(file: File): Promise<Attachment> {
  let mime: string;
  let data: string;
  if (isPdf(file)) {
    if (file.size > PDF_LIMIT) throw new Error('PDF больше 15 МБ');
    mime = 'application/pdf';
    data = base64Of(await readDataUrl(file));
  } else {
    mime = 'image/jpeg';
    data = await toJpeg(file);
  }
  const r = await api.blobPut(mime, data);
  const name = file.name && !/^image\.|^IMG_|^\d+\.jpg$/i.test(file.name) ? file.name : mime === 'application/pdf' ? 'документ.pdf' : 'чек.jpg';
  return { id: r.id, mime, name, size: r.size };
}

/** Показ вложений: запоминаем уже загруженные. */
const cache = new Map<string, string | null>();

export async function blobUrl(id: string): Promise<string | null> {
  if (cache.has(id)) return cache.get(id) ?? null;
  const url = await api.blobUrl(id);
  if (url) cache.set(id, url);
  return url;
}

export function sizeText(n: number): string {
  if (n >= 1024 * 1024) return (n / 1024 / 1024).toFixed(1).replace('.', ',') + ' МБ';
  return Math.max(1, Math.round(n / 1024)) + ' КБ';
}
