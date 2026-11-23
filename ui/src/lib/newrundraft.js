// New run's form, kept while the operator visits another page (Settings,
// Camera, History) and finds it as left on coming back. In memory only: the
// window closed or the page reloaded, the form starts empty again. A run
// started drops it, "Run again" and a settings file replace it.
let kept = null;

/** Keep the form as it stands (plain values, `$state.snapshot`). */
export function keepDraft(draft) {
  kept = structuredClone(draft);
}

/** The form to open on, or null. The pump direction is never brought back:
 *  the tube may have been remounted meanwhile, it is chosen again. */
export function takeDraft() {
  if (!kept) return null;
  const d = structuredClone(kept);
  d.f.direction = null;
  return d;
}

/** Forget the form: the run it set up has started. */
export function dropDraft() {
  kept = null;
}
