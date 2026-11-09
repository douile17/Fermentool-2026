import { describe, expect, it } from 'vitest';
import { deflateRawSync } from 'node:zlib';
import { isDateFormat, readXlsx, unzip, writeXlsx, zip } from './xlsx.js';

describe('zip', () => {
  it('reads back what it stored', async () => {
    expect(await unzip(zip({ 'a.txt': 'hello', 'b/c.xml': '<x>é</x>' }))).toEqual({ 'a.txt': 'hello', 'b/c.xml': '<x>é</x>' });
  });

  it('inflates a deflated entry, as Excel writes them', async () => {
    const files = { 'a.txt': 'x'.repeat(5000) };
    const bytes = zip(files, (b) => new Uint8Array(deflateRawSync(b)));
    expect(bytes.length).toBeLessThan(1000);
    expect(await unzip(bytes)).toEqual(files);
  });

  it('refuses what is not a zip', async () => {
    await expect(unzip(new TextEncoder().encode('From (h);Rate (ml/min)\n0;1'))).rejects.toThrow('not an Excel workbook');
  });
});

describe('isDateFormat', () => {
  it('tells a time format from a number format', () => {
    expect(isDateFormat('h:mm')).toBe(true);
    expect(isDateFormat('[h]:mm:ss')).toBe(true);
    expect(isDateFormat('dd/mm/yyyy')).toBe(true);
    expect(isDateFormat('0.00')).toBe(false);
    expect(isDateFormat('General')).toBe(false);
    expect(isDateFormat('[Red]0.0')).toBe(false);
    expect(isDateFormat('0.0" h"')).toBe(false);
  });
});

describe('readXlsx', () => {
  it('reads the first sheet: text, numbers, gaps, a time-formatted number', async () => {
    const bytes = writeXlsx(
      [
        { name: 'Steps', rows: [[{ text: 'From (h)', bold: true }, 'Rate (ml/min)'], [0, 0.8], [], [{ time: 0.5 }, 2.5]] },
        { name: 'Other', rows: [['ignored']] },
      ],
      (b) => new Uint8Array(deflateRawSync(b)),
    );
    expect(await readXlsx(bytes)).toEqual([['From (h)', 'Rate (ml/min)'], [0, 0.8], [], [{ time: 0.5 }, 2.5]]);
  });

  it('reads shared strings and cells placed by reference, as Excel saves them', async () => {
    const ns = 'http://schemas.openxmlformats.org';
    const bytes = zip({
      'xl/workbook.xml': `<workbook xmlns:r="${ns}/officeDocument/2006/relationships"><sheets><sheet name="S" sheetId="1" r:id="rId3"/></sheets></workbook>`,
      'xl/_rels/workbook.xml.rels': `<Relationships><Relationship Id="rId3" Target="/xl/worksheets/sheet9.xml"/></Relationships>`,
      'xl/sharedStrings.xml': '<sst><si><t>From (h)</t></si><si><r><t>Rate </t></r><r><t>(ml/min)</t></r></si></sst>',
      'xl/styles.xml': '<styleSheet><numFmts><numFmt numFmtId="164" formatCode="0.000"/></numFmts><cellXfs><xf numFmtId="0"/><xf numFmtId="164"/></cellXfs></styleSheet>',
      'xl/worksheets/sheet9.xml':
        '<worksheet><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row>' +
        '<row r="3"><c r="B3" s="1"><v>1.5</v></c></row></sheetData></worksheet>',
    });
    expect(await readXlsx(bytes)).toEqual([['From (h)', 'Rate (ml/min)'], [], [null, 1.5]]);
  });
});
