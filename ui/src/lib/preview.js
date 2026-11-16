// New run's curve preview: only the answer to the latest request counts, and
// the profile is ready only once that answer is in and the steps are right.

/** Numbers each request: `begin()` before sending, `current(id)` when its
 *  answer comes. A preview asked while the steps were right and answered
 *  after they went wrong cleared the error, and the profile read valid. */
export function newestOnly() {
  let n = 0;
  return { begin: () => ++n, current: (id) => id === n };
}

/** The profile can start: a preview drawn, no error, a duration, and no
 *  step out of order (said here, not by the daemon). */
export const profileReady = ({ preview, previewErr, stepsErr = null, durationS }) =>
  !stepsErr && !previewErr && durationS > 0 && preview.length > 0;
