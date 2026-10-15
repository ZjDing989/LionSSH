const fs = require('node:fs/promises');
const path = require('node:path');
const { chromium } = require('playwright');
(async () => {
  const assets = path.join(__dirname, '..', 'assets');
  const svg = await fs.readFile(path.join(assets, 'icon.svg'), 'utf8');
  const browser = await chromium.launch({ channel: 'chrome', headless: true });
  try {
    const page = await browser.newPage({ viewport: { width: 256, height: 256 }, deviceScaleFactor: 1 });
    await page.setContent(`<html><body style="margin:0;background:transparent">${svg}</body></html>`);
    const png = await page.screenshot({ path: path.join(assets, 'icon.png'), omitBackground: true });
    // ICO can embed PNG directly; width/height zero denote a 256-pixel icon.
    const header = Buffer.alloc(22);
    header.writeUInt16LE(1, 2); header.writeUInt16LE(1, 4);
    header.writeUInt16LE(1, 10); header.writeUInt16LE(32, 12);
    header.writeUInt32LE(png.length, 14); header.writeUInt32LE(22, 18);
    await fs.writeFile(path.join(assets, 'icon.ico'), Buffer.concat([header, png]));
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
