import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import './styles/global.css';

// Apply any previously-saved theme preference before the first paint to
// avoid a flash of the wrong scheme.
const saved = localStorage.getItem('bugjuice-theme');
if (saved === 'light' || saved === 'dark') {
  document.documentElement.setAttribute('data-theme', saved);
}

/**
 * Read the user's actual Windows system accent color from the Rust
 * backend (which reads the registry) and apply it as CSS variables.
 * Falls back to BugJuice green if the backend isn't available.
 */
async function applySystemAccent(): Promise<void> {
  const root = document.documentElement;
  if (!('__TAURI_INTERNALS__' in window)) {
    root.dataset.systemAccent = 'fallback';
    return;
  }
  try {
    const { invoke } = await import('@tauri-apps/api/core');
    const hex: string = await invoke('get_accent_color');
    root.style.setProperty('--accent', hex);
    root.style.setProperty(
      '--accent-hover',
      `color-mix(in srgb, ${hex} 82%, black 18%)`,
    );
    root.style.setProperty(
      '--accent-soft',
      `color-mix(in srgb, ${hex} 14%, transparent)`,
    );
    root.dataset.systemAccent = 'detected';
  } catch {
    root.dataset.systemAccent = 'fallback';
  }
}

applySystemAccent();
document.addEventListener('visibilitychange', () => {
  if (!document.hidden) applySystemAccent();
});

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
