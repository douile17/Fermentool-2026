import { afterEach, describe, expect, it } from 'vitest';
import { dropDraft, keepDraft, takeDraft } from './newrundraft.js';

const form = () => ({ f: { name: 'E. coli fed-batch', direction: 'cw', steps: [{ h: '0', m: '0', value: 5 }] }, step: 3, tlSeen: true });

describe('New run draft', () => {
  afterEach(() => dropDraft());

  it('starts with no draft', () => {
    expect(takeDraft()).toBeNull();
  });

  it('gives the form back as left, after a visit to another page', () => {
    keepDraft(form());
    const d = takeDraft();
    expect(d.f.name).toBe('E. coli fed-batch');
    expect(d.f.steps).toEqual([{ h: '0', m: '0', value: 5 }]);
    expect(d.step).toBe(3);
    expect(d.tlSeen).toBe(true);
  });

  it('never brings the pump direction back', () => {
    keepDraft(form());
    expect(takeDraft().f.direction).toBeNull();
  });

  it('keeps a copy: the form edited after does not change the draft', () => {
    const live = form();
    keepDraft(live);
    live.f.name = 'changed';
    live.f.steps.push({ h: '1', m: '0', value: 9 });
    const d = takeDraft();
    expect(d.f.name).toBe('E. coli fed-batch');
    expect(d.f.steps).toHaveLength(1);
    d.f.name = 'again';
    expect(takeDraft().f.name).toBe('E. coli fed-batch');
  });

  it('is gone once the run started', () => {
    keepDraft(form());
    dropDraft();
    expect(takeDraft()).toBeNull();
  });
});
