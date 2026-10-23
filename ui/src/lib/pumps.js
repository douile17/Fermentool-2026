// Settings > Pumps: one serial cable (RS-485), every LabQ on it at its own
// MODBUS address. What the page says about the cable and each pump, from
// the status frame; colour only for a problem (docs/DESIGN.md).

/** The pump models on the cable (mirrors the daemon's `PumpModel`):
 *  `maxRpm` the default top speed, `ceiling` the most a max rpm may be set
 *  to, `maxAddress` the highest MODBUS address. */
export const MODELS = [
  { id: 'labq', label: 'LabQ', maxRpm: 350, ceiling: 350, maxAddress: 247 },
  { id: 'labv', label: 'LabV', maxRpm: 150, ceiling: 600, maxAddress: 32 },
  { id: 'pds2', label: 'PDS II', maxRpm: 600, ceiling: 600, maxAddress: 32 },
];

/** A model by id; an unknown or missing id is a LabQ (old configs). */
export const modelOf = (id) => MODELS.find((m) => m.id === id) ?? MODELS[0];

/** Same physical pump: same address, and on a PDS II (one address, several
 *  heads) the same head. Mirrors `same_pump` (config.rs). */
export function samePump(a, b) {
  if (Number(a.address) !== Number(b.address)) return false;
  if (a.model === 'pds2' && b.model === 'pds2') return Number(a.head) === Number(b.head);
  return true;
}

/** What the daemon would refuse in a pump's `address`, `head` and `maxRpm`
 *  (null = the model's default); null if fine. Mirrors `pump_problem`. */
export function pumpProblem({ model, address, head = 1, maxRpm = null }) {
  const m = modelOf(model);
  const a = Number(address);
  if (!Number.isInteger(a) || a < 1 || a > m.maxAddress) return `The MODBUS address must be 1 to ${m.maxAddress} on a ${m.label}.`;
  if (m.id === 'pds2') {
    const h = Number(head);
    if (!Number.isInteger(h) || h < 1 || h > 8) return 'The PDS II head must be 1 to 8.';
  }
  if (maxRpm != null && maxRpm !== '') {
    const r = Number(maxRpm);
    if (!(r >= 0.1 && r <= m.ceiling)) return `The max rpm must be between 0.1 and ${m.ceiling} on a ${m.label}.`;
  }
  return null;
}

/** The question under a pump that does not answer. */
export function notAnsweringHint({ model, address, head = 1 }) {
  const m = modelOf(model);
  if (m.id === 'pds2')
    return `Is the PDS II switched on, at address ${address}, 9600 baud, and head ${head} set to independent operation?`;
  return `Is the ${m.label} switched on, at address ${address} and 9600 baud in its own menu?`;
}

/** The cable is up and is a real port (the simulator talks to no pump). */
export const lineUp = (s) => !!s && s.serial_ok !== false && !s.simulator;

/** The cable's state header (Settings > Pumps, mockup B of 2026-10-22):
 *  `{ tone, word, text }`, a big word and its detail. tone '' (connected,
 *  green), 'warn' (a pump does not answer), 'bad' (the port does not
 *  answer), 'idle' (not connected) or 'sim'. `answering` / `total`: how
 *  many pumps on the cable answer. */
export function lineState(s, { answering, total }) {
  if (!s) return { tone: 'idle', word: 'Asking…', text: '' };
  if (s.simulator && s.allow_simulator) return { tone: 'sim', word: 'Simulator', text: 'No cable, test runs only' };
  if (s.simulator) return { tone: 'idle', word: 'Not connected', text: "Pick the cable's port and Connect" };
  if (s.serial_ok === false)
    return { tone: 'bad', word: 'Not answering', text: `${s.transport || 'The port'}: check the USB adapter, the cable and that the pumps are on, then Connect` };
  const n = `${answering} pump${answering === 1 ? '' : 's'} of ${total} answer${answering === 1 ? 's' : ''}`;
  return { tone: answering < total ? 'warn' : '', word: 'Connected', text: `${s.transport}, ${n}` };
}

/** One pump's state: '' answers, 'bad' does not, 'idle' not asked (the
 *  cable is down, or nothing asked yet). */
export function pumpTone({ up, responding }) {
  if (!up || responding == null) return 'idle';
  return responding ? '' : 'bad';
}

/** The feed pump answers when the cable is up and its writes get through. */
export const feedResponding = (s) => (lineUp(s) ? s.pump_confirmed !== false && (s.write_fails ?? 0) < 3 : null);

const maxRpmKey = (v) => (v == null || v === '' ? null : Number(v));

/** Whether Save must reconnect the cable: its port, baud or the feed
 *  pump's address, model, head or max rpm changed (the daemon applies those
 *  on a reconnect). */
export const needsReconnect = (saved, now) =>
  !!saved &&
  (saved.path !== now.path ||
    Number(saved.baud) !== Number(now.baud) ||
    Number(saved.feedAddress) !== Number(now.feedAddress) ||
    (saved.feedModel ?? 'labq') !== (now.feedModel ?? 'labq') ||
    Number(saved.feedHead ?? 1) !== Number(now.feedHead ?? 1) ||
    maxRpmKey(saved.feedMaxRpm) !== maxRpmKey(now.feedMaxRpm));

/** Settings > Pumps asks the refill pump itself (`POST /api/refill/ping`):
 *  outside a run nothing else does, while the daemon probes the feed pump
 *  every second. Not during a run (the status frame carries it, and a read
 *  left unanswered would hold the bus), nor with the cable down. */
export const pingWanted = (s) => lineUp(s) && !s.active;

/** When to ask again: soon while it does not answer, so a cable plugged
 *  back shows both pumps together (2026-10-22: "1 pump of 2" for up to
 *  15 s, the refill pump's last answer dating from the unplugged cable). */
export const pingDelayMs = (responding) => (responding === false ? 3_000 : 15_000);

/** Ask at once when the feed pump comes back: it shares the cable, so the
 *  refill pump's last answer is stale. */
export const feedCameBack = (before, now) => before !== true && now === true;
