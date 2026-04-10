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
 * Read the user's actual Windows system accent color and apply it as
 * a CSS variable so the entire UI follows it. The CSS `AccentColor`
 * keyword is supposed to do this on its own, but support inside
 * `color-mix()` is patchy in current Chromium — probing it via JS and
 * setting a real rgb() value sidesteps the issue.
 *
 * Re-probes on visibility change so changing the Windows accent in
 * Settings → Personalization → Colors propagates back to BugJuice as
 * soon as you click back to the window.
 */
function applySystemAccent(): void {
  // Probe AccentColor by setting it on a hidden element and reading
  // back the computed color. If the browser doesn't recognize the
  // keyword we get a default (usually rgb(0, 0, 0) inherited or empty),
  // which we ignore — the CSS BugJuice-green fallback wins.
  const probe = document.createElement('span');
  probe.style.cssText =
    'color: AccentColor; position: absolute; visibility: hidden; pointer-events: none;';
  document.body.appendChild(probe);
  const accentColor = getComputedStyle(probe).color;
  probe.style.color = 'AccentColorText';
  const accentText = getComputedStyle(probe).color;
  document.body.removeChild(probe);

  const looksValid = (c: string) =>
    c &&
    c !== 'rgba(0, 0, 0, 0)' &&
    c !== 'rgb(0, 0, 0)' &&
    c !== 'transparent';

  const root = document.documentElement;
  if (looksValid(accentColor)) {
    root.style.setProperty('--accent', accentColor);
    if (looksValid(accentText)) {
      root.style.setProperty('--accent-text', accentText);
    }
    // Mark for debugging — open DevTools and you can see if probing succeeded.
    root.dataset.systemAccent = 'detected';
  } else {
    root.dataset.systemAccent = 'fallback';
  }
}

// Run before React renders so the first paint already has the right accent.
applySystemAccent();
// Re-apply when the user comes back from changing Windows colors.
document.addEventListener('visibilitychange', () => {
  if (!document.hidden) applySystemAccent();
});

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
