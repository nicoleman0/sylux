// Synthetic fixtures generated from PDF objects; no external documents or assets are used.
import { deflateSync } from 'node:zlib';

type PdfObject = { id: number; body: Buffer };

function pdf(objects: PdfObject[], rootId = 1): Buffer {
  const chunks: Buffer[] = [Buffer.from('%PDF-1.7\n')];
  const offsets = new Map<number, number>();
  let length = chunks[0].length;

  for (const object of objects) {
    offsets.set(object.id, length);
    const chunk = Buffer.concat([
      Buffer.from(`${object.id} 0 obj\n`),
      object.body,
      Buffer.from('\nendobj\n'),
    ]);
    chunks.push(chunk);
    length += chunk.length;
  }

  const maxId = objects.reduce((highest, { id }) => Math.max(highest, id), 0);
  const xrefOffset = length;
  let xref = `xref\n0 ${maxId + 1}\n0000000000 65535 f \n`;
  for (let id = 1; id <= maxId; id++) {
    const offset = offsets.get(id);
    xref += offset === undefined ? '0000000000 00000 f \n' : `${String(offset).padStart(10, '0')} 00000 n \n`;
  }
  xref += `trailer\n<< /Size ${maxId + 1} /Root ${rootId} 0 R >>\nstartxref\n${xrefOffset}\n%%EOF\n`;
  chunks.push(Buffer.from(xref));
  return Buffer.concat(chunks);
}

function stream(dictionary: string, data: Buffer): Buffer {
  return Buffer.concat([
    Buffer.from(`<< ${dictionary} /Length ${data.length} >>\nstream\n`),
    data,
    Buffer.from('\nendstream'),
  ]);
}

function textContent(text: string, width: number, height: number): Buffer {
  const escaped = text.replaceAll('\\', '\\\\').replaceAll('(', '\\(').replaceAll(')', '\\)');
  return Buffer.from(`BT /F1 18 Tf 48 ${height - 72} Td (${escaped}) Tj ET\n`);
}

function pagesPdf(pages: Array<{ width: number; height: number; content: Buffer; resources?: string }>): Buffer {
  const fontId = 3;
  const firstPageId = 4;
  const pageIds = pages.map((_, index) => firstPageId + index * 2);
  const objects: PdfObject[] = [
    { id: 1, body: Buffer.from('<< /Type /Catalog /Pages 2 0 R >>') },
    { id: 2, body: Buffer.from(`<< /Type /Pages /Kids [${pageIds.map(id => `${id} 0 R`).join(' ')}] /Count ${pages.length} >>`) },
    { id: fontId, body: Buffer.from('<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>') },
  ];

  pages.forEach((page, index) => {
    const pageId = pageIds[index];
    const contentId = pageId + 1;
    objects.push({
      id: pageId,
      body: Buffer.from(`<< /Type /Page /Parent 2 0 R /MediaBox [0 0 ${page.width} ${page.height}] /Resources << /Font << /F1 ${fontId} 0 R >>${page.resources ?? ''} >> /Contents ${contentId} 0 R >>`),
    });
    objects.push({ id: contentId, body: stream('', page.content) });
  });

  return pdf(objects);
}

/** One-page 4100×4100 grayscale scan with a dark center target and a tiled gray field. */
export function scannedPdf(): Buffer {
  const width = 4100;
  const height = 4100;
  const pixels = Buffer.alloc(width * height);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const inTarget = x >= 1400 && x < 2700 && y >= 1400 && y < 2700;
      pixels[y * width + x] = inTarget ? 28 : ((Math.floor(x / 32) + Math.floor(y / 32)) % 2 === 0 ? 208 : 232);
    }
  }

  const imageId = 3;
  const contentId = 5;
  const content = Buffer.from('q 612 0 0 612 0 90 cm /Scan Do Q\n');
  return pdf([
    { id: 1, body: Buffer.from('<< /Type /Catalog /Pages 2 0 R >>') },
    { id: 2, body: Buffer.from('<< /Type /Pages /Kids [4 0 R] /Count 1 >>') },
    { id: imageId, body: stream(`/Type /XObject /Subtype /Image /Width ${width} /Height ${height} /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode`, deflateSync(pixels)) },
    { id: 4, body: Buffer.from(`<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /XObject << /Scan ${imageId} 0 R >> >> /Contents ${contentId} 0 R >>`) },
    { id: contentId, body: stream('', content) },
  ]);
}

/** Three searchable pages with portrait, landscape, and narrow portrait dimensions. */
export function mixedPdf(): Buffer {
  return pagesPdf([
    { width: 612, height: 792, content: textContent('Portrait page: searchable fixture text.', 612, 792) },
    { width: 792, height: 612, content: textContent('Landscape page: searchable fixture text.', 792, 612) },
    { width: 420, height: 720, content: textContent('Mixed page: searchable fixture text.', 420, 720) },
  ]);
}

/** A generated multi-page PDF with a short text label on each page. */
export function manyPagesPdf(count = 40): Buffer {
  if (!Number.isSafeInteger(count) || count < 1) {
    throw new RangeError('count must be a positive safe integer');
  }
  return pagesPdf(Array.from({ length: count }, (_, index) => ({
    width: 612,
    height: 792,
    content: textContent(`Page ${index + 1} of ${count}`, 612, 792),
  })));
}

/** A valid one-page PDF whose page content stream is empty. */
export function blankPdf(): Buffer {
  return pagesPdf([{ width: 612, height: 792, content: Buffer.alloc(0) }]);
}

/** A saved red square annotation whose appearance must survive read-only preview. */
export function annotatedPdf(): Buffer {
  return pdf([
    { id: 1, body: Buffer.from('<< /Type /Catalog /Pages 2 0 R >>') },
    { id: 2, body: Buffer.from('<< /Type /Pages /Kids [3 0 R] /Count 1 >>') },
    { id: 3, body: Buffer.from('<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> /Contents 4 0 R /Annots [5 0 R] >>') },
    { id: 4, body: stream('', Buffer.alloc(0)) },
    { id: 5, body: Buffer.from('<< /Type /Annot /Subtype /Square /Rect [50 100 250 300] /F 4 /AP << /N 6 0 R >> >>') },
    { id: 6, body: stream('/Type /XObject /Subtype /Form /BBox [0 0 200 200] /Resources << >>', Buffer.from('1 0 0 rg 0 0 200 200 re f')) },
  ]);
}
