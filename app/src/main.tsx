import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
// Шрифты и экран входа — из общего модуля kit; остальной вид «Кармана» пока свой (docs/kit.md).
import '@primus-fixer/kit/ui/fonts';
import '@primus-fixer/kit/ui/lock.css';
import './styles.css';
import App from './App';

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
