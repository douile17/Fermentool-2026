// Arrow keys in a group of choice cards (`role="radiogroup"` holding
// `role="radio"` buttons) move the choice, as they do in native radios: the
// next enabled one takes the focus and is picked. Tab enters the group on
// the checked card (tabindex 0) and leaves it in one press.
export function radioKeys(e) {
  const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[e.key];
  if (!step) return;
  const radios = [...e.currentTarget.querySelectorAll('[role="radio"]:not(:disabled)')];
  const i = radios.indexOf(document.activeElement);
  if (i < 0) return;
  e.preventDefault();
  const next = radios[(i + step + radios.length) % radios.length];
  next.focus();
  next.click();
}

// The tabindex of a card: the checked one, or the first when none is.
export const rovingTab = (checked, first) => (checked || first ? 0 : -1);
