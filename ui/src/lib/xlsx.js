// The least of an Excel workbook (.xlsx) to read a sheet of numbers and to
// write a template: a .xlsx is a zip of XML parts. No library: the one that
// reads them pulls four little-known packages in, and the lab PC is offline.
// Reading inflates with the browser's own DecompressionStream; writing stores
// the parts uncompressed, which Excel opens as any other workbook.

// --- zip --------------------------------------------------------------------

const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();

function crc32(bytes) {
  let c = 0xffffffff;
  for (const b of bytes) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

/** Zip `files` ({ path: string }) into bytes. Stored, unless `deflate` (a
 *  raw-deflate function, the tests' zlib) is given. */
export function zip(files, deflate = null) {
  const enc = new TextEncoder();
  const locals = [];
  const centrals = [];
  let offset = 0;
  for (const [path, text] of Object.entries(files)) {
    const name = enc.encode(path);
    const raw = enc.encode(text);
    const data = deflate ? deflate(raw) : raw;
    const method = deflate ? 8 : 0;
    const crc = crc32(raw);
    const local = new Uint8Array(30 + name.length + data.length);
    const lv = new DataView(local.buffer);
    lv.setUint32(0, 0x04034b50, true);
    lv.setUint16(4, 20, true);
    lv.setUint16(8, method, true);
    lv.setUint32(14, crc, true);
    lv.setUint32(18, data.length, true);
    lv.setUint32(22, raw.length, true);
    lv.setUint16(26, name.length, true);
    local.set(name, 30);
    local.set(data, 30 + name.length);
    const central = new Uint8Array(46 + name.length);
    const cv = new DataView(central.buffer);
    cv.setUint32(0, 0x02014b50, true);
    cv.setUint16(4, 20, true);
    cv.setUint16(6, 20, true);
    cv.setUint16(10, method, true);
    cv.setUint32(16, crc, true);
    cv.setUint32(20, data.length, true);
    cv.setUint32(24, raw.length, true);
    cv.setUint16(28, name.length, true);
    cv.setUint32(42, offset, true);
    central.set(name, 46);
    locals.push(local);
    centrals.push(central);
    offset += local.length;
  }
  const dirSize = centrals.reduce((n, c) => n + c.length, 0);
  const end = new Uint8Array(22);
  const ev = new DataView(end.buffer);
  ev.setUint32(0, 0x06054b50, true);
  ev.setUint16(8, centrals.length, true);
  ev.setUint16(10, centrals.length, true);
  ev.setUint32(12, dirSize, true);
  ev.setUint32(16, offset, true);
  const out = new Uint8Array(offset + dirSize + 22);
  let at = 0;
  for (const part of [...locals, ...centrals, end]) {
    out.set(part, at);
    at += part.length;
  }
  return out;
}

async function inflateRaw(bytes) {
  const stream = new Blob([bytes]).stream().pipeThrough(new DecompressionStream('deflate-raw'));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

/** The text of every entry of a zip, by path. Throws on what is not a zip. */
export async function unzip(buffer) {
  const bytes = new Uint8Array(buffer);
  const v = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let eocd = -1;
  for (let i = bytes.length - 22; i >= Math.max(0, bytes.length - 65557); i--) {
    if (v.getUint32(i, true) === 0x06054b50) {
      eocd = i;
      break;
    }
  }
  if (eocd < 0) throw new Error('not an Excel workbook (.xlsx)');
  const count = v.getUint16(eocd + 10, true);
  let p = v.getUint32(eocd + 16, true);
  const dec = new TextDecoder();
  const out = {};
  for (let i = 0; i < count; i++) {
    if (p + 46 > bytes.length || v.getUint32(p, true) !== 0x02014b50) throw new Error('damaged workbook');
    const method = v.getUint16(p + 10, true);
    const size = v.getUint32(p + 20, true);
    const nameLen = v.getUint16(p + 28, true);
    const extraLen = v.getUint16(p + 30, true);
    const commentLen = v.getUint16(p + 32, true);
    const local = v.getUint32(p + 42, true);
    const name = dec.decode(bytes.subarray(p + 46, p + 46 + nameLen));
    p += 46 + nameLen + extraLen + commentLen;
    if (local + 30 > bytes.length || v.getUint32(local, true) !== 0x04034b50) throw new Error('damaged workbook');
    const start = local + 30 + v.getUint16(local + 26, true) + v.getUint16(local + 28, true);
    const data = bytes.subarray(start, start + size);
    if (method === 0) out[name] = dec.decode(data);
    else if (method === 8) out[name] = dec.decode(await inflateRaw(data));
    // any other method: an entry we do not need, or the read below says what is missing
  }
  return out;
}

// --- XML --------------------------------------------------------------------

const unescapeXml = (s) =>
  s.replace(/&(#x[0-9a-f]+|#\d+|lt|gt|amp|quot|apos);/gi, (_, e) => {
    if (e[0] === '#') return String.fromCodePoint(e[1] === 'x' || e[1] === 'X' ? parseInt(e.slice(2), 16) : parseInt(e.slice(1), 10));
    return { lt: '<', gt: '>', amp: '&', quot: '"', apos: "'" }[e.toLowerCase()];
  });
const escapeXml = (s) => String(s).replace(/[<>&"]/g, (c) => ({ '<': '&lt;', '>': '&gt;', '&': '&amp;', '"': '&quot;' })[c]);

function attrs(s) {
  const out = {};
  for (const m of s.matchAll(/([\w:]+)\s*=\s*"([^"]*)"/g)) out[m[1]] = unescapeXml(m[2]);
  return out;
}

// All the text runs of a string item (`<si>` or `<is>`), joined.
const textOf = (xml) => [...xml.matchAll(/<t(?:\s[^>]*)?>([\s\S]*?)<\/t>/g)].map((m) => unescapeXml(m[1])).join('');

// Built-in number formats that show a date or a time.
const DATE_FMT_IDS = new Set([14, 15, 16, 17, 18, 19, 20, 21, 22, 27, 30, 36, 45, 46, 47, 50, 57]);

/** A number format code shows a date or a time: d, m, y, h or s outside
 *  quoted text, brackets and escapes ("[h]:mm", not "0.00" nor "[Red]0"). */
export function isDateFormat(code) {
  const bare = code.replace(/"[^"]*"/g, '').replace(/\[[^\]]*\]/g, '').replace(/\\./g, '').replace(/_./g, '');
  return /[dmyhs]/i.test(bare.replace(/General/gi, ''));
}

/** Rows of the workbook's first sheet: arrays of cell values, a number, a
 *  string, a boolean, null (empty), or `{ time: n }` for a number Excel
 *  shows as a date or a time (12:30 is 0.52 underneath). */
export async function readXlsx(buffer) {
  const parts = await unzip(buffer);
  const book = parts['xl/workbook.xml'];
  if (!book) throw new Error('not an Excel workbook (.xlsx)');
  const first = book.match(/<sheet\b[^>]*>/);
  if (!first) throw new Error('the workbook has no sheet');
  const rid = attrs(first[0])['r:id'];
  const rels = parts['xl/_rels/workbook.xml.rels'] ?? '';
  const rel = [...rels.matchAll(/<Relationship\b[^>]*>/g)].map((m) => attrs(m[0])).find((r) => r.Id === rid);
  const target = rel ? rel.Target.replace(/^\//, '').replace(/^(?!xl\/)/, 'xl/') : 'xl/worksheets/sheet1.xml';
  const sheet = parts[target];
  if (sheet == null) throw new Error('the first sheet is missing from the workbook');

  const shared = [...(parts['xl/sharedStrings.xml'] ?? '').matchAll(/<si>([\s\S]*?)<\/si>/g)].map((m) => textOf(m[1]));

  const styles = parts['xl/styles.xml'] ?? '';
  const custom = {};
  for (const m of styles.matchAll(/<numFmt\b[^>]*>/g)) {
    const a = attrs(m[0]);
    custom[+a.numFmtId] = a.formatCode ?? '';
  }
  const xfs = styles.match(/<cellXfs\b[^>]*>([\s\S]*?)<\/cellXfs>/);
  const dateStyle = xfs
    ? [...xfs[1].matchAll(/<xf\b[^>]*>/g)].map((m) => {
        const id = +(attrs(m[0]).numFmtId ?? 0);
        return DATE_FMT_IDS.has(id) || (id in custom && isDateFormat(custom[id]));
      })
    : [];

  const rows = [];
  let rowNo = 0;
  for (const rm of sheet.matchAll(/<row\b([^>]*?)(?:\/>|>([\s\S]*?)<\/row>)/g)) {
    const ra = attrs(rm[1]);
    rowNo = ra.r ? +ra.r : rowNo + 1;
    const cells = [];
    let col = 0;
    for (const cm of (rm[2] ?? '').matchAll(/<c\b([^>]*?)(?:\/>|>([\s\S]*?)<\/c>)/g)) {
      const a = attrs(cm[1]);
      if (a.r) {
        const letters = a.r.match(/^[A-Z]+/)[0];
        col = [...letters].reduce((n, ch) => n * 26 + ch.charCodeAt(0) - 64, 0) - 1;
      }
      const body = cm[2] ?? '';
      const vm = body.match(/<v>([\s\S]*?)<\/v>/);
      const raw = vm ? unescapeXml(vm[1]) : null;
      let value = null;
      if (a.t === 's') value = raw == null ? null : (shared[+raw] ?? null);
      else if (a.t === 'inlineStr') value = textOf(body);
      else if (a.t === 'str') value = raw;
      else if (a.t === 'b') value = raw === '1';
      else if (a.t === 'e') value = raw;
      else if (raw != null && raw !== '') {
        const n = Number(raw);
        value = dateStyle[+(a.s ?? 0)] ? { time: n } : n;
      }
      if (typeof value === 'string' && value === '') value = null;
      cells[col] = value;
      col++;
    }
    for (let i = 0; i < cells.length; i++) if (cells[i] === undefined) cells[i] = null;
    rows[rowNo - 1] = cells;
  }
  for (let i = 0; i < rows.length; i++) if (rows[i] === undefined) rows[i] = [];
  return rows;
}

// --- writing ----------------------------------------------------------------

const colName = (i) => (i < 26 ? String.fromCharCode(65 + i) : colName(Math.floor(i / 26) - 1) + String.fromCharCode(65 + (i % 26)));

function sheetXml({ rows, widths = [], freezeHeader = false, validations = [] }) {
  const cols = widths.length
    ? `<cols>${widths.map((w, i) => `<col min="${i + 1}" max="${i + 1}" width="${w}" customWidth="1"/>`).join('')}</cols>`
    : '';
  const view = freezeHeader
    ? '<sheetViews><sheetView workbookViewId="0"><pane ySplit="1" topLeftCell="A2" activePane="bottomLeft" state="frozen"/></sheetView></sheetViews>'
    : '';
  const data = rows
    .map((row, r) => {
      const cells = row
        .map((v, c) => {
          const ref = `${colName(c)}${r + 1}`;
          if (v == null) return '';
          if (typeof v === 'number') return `<c r="${ref}"><v>${v}</v></c>`;
          if (typeof v === 'object' && 'time' in v) return `<c r="${ref}" s="2"><v>${v.time}</v></c>`;
          const bold = typeof v === 'object' && v.bold;
          const text = typeof v === 'object' ? v.text : v;
          return `<c r="${ref}" t="inlineStr"${bold ? ' s="1"' : ''}><is><t xml:space="preserve">${escapeXml(text)}</t></is></c>`;
        })
        .join('');
      return `<row r="${r + 1}">${cells}</row>`;
    })
    .join('');
  const dv = validations.length
    ? `<dataValidations count="${validations.length}">${validations
        .map(
          (d) =>
            `<dataValidation type="${d.type}" operator="${d.operator}" allowBlank="1" showInputMessage="1" showErrorMessage="1" errorTitle="${escapeXml(d.errorTitle)}" error="${escapeXml(d.error)}" promptTitle="${escapeXml(d.promptTitle)}" prompt="${escapeXml(d.prompt)}" sqref="${d.sqref}"><formula1>${d.formula}</formula1></dataValidation>`,
        )
        .join('')}</dataValidations>`
    : '';
  return `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">${view}${cols}<sheetData>${data}</sheetData>${dv}</worksheet>`;
}

/**
 * A workbook's bytes. `sheets`: [{ name, rows, widths?, freezeHeader?,
 * validations? }]; a cell is a number, a string, `{ text, bold }` or
 * `{ time }` (a number shown h:mm, for the tests).
 */
export function writeXlsx(sheets, deflate = null) {
  const ns = 'http://schemas.openxmlformats.org';
  const files = {
    '[Content_Types].xml': `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="${ns}/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>${sheets
      .map((_, i) => `<Override PartName="/xl/worksheets/sheet${i + 1}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>`)
      .join('')}</Types>`,
    '_rels/.rels': `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="${ns}/package/2006/relationships"><Relationship Id="rId1" Type="${ns}/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>`,
    'xl/workbook.xml': `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="${ns}/spreadsheetml/2006/main" xmlns:r="${ns}/officeDocument/2006/relationships"><sheets>${sheets
      .map((s, i) => `<sheet name="${escapeXml(s.name)}" sheetId="${i + 1}" r:id="rId${i + 1}"/>`)
      .join('')}</sheets></workbook>`,
    'xl/_rels/workbook.xml.rels': `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="${ns}/package/2006/relationships">${sheets
      .map((_, i) => `<Relationship Id="rId${i + 1}" Type="${ns}/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet${i + 1}.xml"/>`)
      .join('')}<Relationship Id="rId${sheets.length + 1}" Type="${ns}/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>`,
    'xl/styles.xml': `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<styleSheet xmlns="${ns}/spreadsheetml/2006/main"><fonts count="2"><font><sz val="11"/><name val="Calibri"/></font><font><b/><sz val="11"/><name val="Calibri"/></font></fonts><fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills><borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs><cellXfs count="3"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/><xf numFmtId="0" fontId="1" fillId="0" borderId="0" xfId="0" applyFont="1"/><xf numFmtId="20" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/></cellXfs></styleSheet>`,
  };
  sheets.forEach((s, i) => (files[`xl/worksheets/sheet${i + 1}.xml`] = sheetXml(s)));
  return zip(files, deflate);
}
