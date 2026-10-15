const fs = require('node:fs/promises');
const path = require('node:path');
const { chromium } = require('playwright');

// Rebuild the committed macOS icon from the project's vector logo.
// This maintenance script needs a Playwright Chromium browser or installed Chrome.
async function main() {
  const assets = path.join(__dirname, '..', 'assets');
  const svg = await fs.readFile(path.join(assets, 'icon.svg'), 'utf8');
  let browser;
  try { browser = await chromium.launch({ headless: true }); }
  catch { browser = await chromium.launch({ channel: 'chrome', headless: true }); }
  try {
    const page = await browser.newPage({ deviceScaleFactor: 1 });
    await page.setContent(`<html><body style="margin:0;background:transparent">${svg}<style>svg{display:block;width:100vw;height:100vh}</style></body></html>`);
    const chunks = [];
    for (const [type, size] of [['ic07', 128], ['ic08', 256], ['ic09', 512], ['ic10', 1024]]) {
      await page.setViewportSize({ width: size, height: size });
      const png = await page.screenshot({ omitBackground: true });
      const header = Buffer.alloc(8);
      header.write(type, 0, 4, 'ascii');
      header.writeUInt32BE(png.length + header.length, 4);
      chunks.push(header, png);
    }
    const header = Buffer.alloc(8);
    header.write('icns', 0, 4, 'ascii');
    header.writeUInt32BE(header.length + chunks.reduce((sum, chunk) => sum + chunk.length, 0), 4);
    await fs.writeFile(path.join(assets, 'icon.icns'), Buffer.concat([header, ...chunks]));
  } finally { await browser.close(); }
}

main().catch(error => { console.error(error.message); process.exitCode = 1; });
