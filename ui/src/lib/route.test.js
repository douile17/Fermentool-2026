import { describe, expect, it } from 'vitest';
import { parseHash } from './route.js';

const S = ['pumps', 'balance', 'camera', 'about'];

describe('parseHash', () => {
  it('opens a Settings section', () => {
    expect(parseHash('#settings/pumps', S)).toEqual({ route: 'settings', section: 'pumps' });
    expect(parseHash('#/Settings/About', S)).toEqual({ route: 'settings', section: 'about' });
  });
  it('opens Settings alone for an unknown section', () => {
    expect(parseHash('#settings/nope', S)).toEqual({ route: 'settings' });
  });
  it('opens a tab of the pump panel', () => {
    expect(parseHash('#history', S)).toEqual({ route: 'pump', tab: 'history' });
  });
  it('opens the Camera page, and its Settings section', () => {
    expect(parseHash('#camera', S)).toEqual({ route: 'camera' });
    expect(parseHash('#settings/camera', S)).toEqual({ route: 'settings', section: 'camera' });
  });
  it('ignores anything else', () => {
    expect(parseHash('', S)).toBeNull();
    expect(parseHash('#new', S)).toBeNull();
    expect(parseHash('#whatever', S)).toBeNull();
  });
});
