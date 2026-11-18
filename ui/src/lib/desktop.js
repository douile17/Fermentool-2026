// The desktop window (Tauri, WebView2) behaves as an application, not a web
// page: no browser menu on a right click, no reload on F5, no image dragged
// out of it. In a browser (`npm run dev`, a tab on the daemon) nothing changes.
import { inDesktop } from './savefile.js';

/** Whether a right click keeps the menu: in a field (paste), or on a text
 *  selection (copy). Anywhere else the browser's menu (Back, Reload, Print,
 *  Inspect) has nothing to do in an application. */
export function keepsMenu(target, selectionText = '') {
  if (selectionText.trim()) return true;
  const el = target?.closest ? target : target?.parentElement;
  return !!el?.closest?.('input, textarea, select, [contenteditable]:not([contenteditable="false"])');
}

/** A reload key: F5, Ctrl+R, Ctrl+Shift+R, Ctrl+F5. Reloading the window
 *  drops what is being typed and shows "reconnecting…" for nothing. */
export const isReloadKey = (e) =>
  e.key === 'F5' || (!!(e.ctrlKey || e.metaKey) && !e.altKey && (e.key === 'r' || e.key === 'R'));

/** Install the application behaviour on `w` when it is the desktop window.
 *  Returns whether it did. */
export function desktopBehaviour(w = window) {
  if (!inDesktop(w)) return false;
  const doc = w.document;
  doc.documentElement.classList.add('desktop');
  doc.addEventListener('contextmenu', (e) => {
    if (!keepsMenu(e.target, String(w.getSelection?.() ?? ''))) e.preventDefault();
  });
  doc.addEventListener('keydown', (e) => {
    if (isReloadKey(e)) e.preventDefault();
  });
  doc.addEventListener('dragstart', (e) => {
    if (e.target?.tagName === 'IMG' || e.target?.tagName === 'A') e.preventDefault();
  });
  return true;
}
