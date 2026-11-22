import { access, copyFile, mkdir, readFile, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const brand = path.join(root, 'assets', 'brand');
const require = createRequire(path.join(root, 'installer', 'ui', 'package.json'));
const { chromium } = require('@playwright/test');
const { values } = parseArgs({ options: { browser: { type: 'string' } } });
const mark = await readFile(path.join(brand, '05-Source', 'Glide-Dock-Mark.svg'), 'utf8');
const wordmark = await readFile(path.join(brand, '05-Source', 'Glide-Dock-Wordmark.svg'), 'utf8');
for (const svg of [mark, wordmark]) {
  if (/<(?:image|text|foreignObject|script)\b|data:image\//i.test(svg)) {
    throw new Error('Logo masters must use vector paths without raster images, fonts, or scripts.');
  }
}
const inner = svg => svg.replace(/^[\s\S]*?<svg\b[^>]*>/, '').replace(/<\/svg>\s*$/, '')
  .replace(/<(title|desc)\b[^>]*>[\s\S]*?<\/\1>/g, '').trim();
const [, wordWidth, wordHeight] = wordmark.match(/viewBox="0 0 ([\d.]+) ([\d.]+)"/) ?? [];
if (!wordWidth || !wordHeight) throw new Error('The outlined wordmark needs a valid viewBox.');
const wordScale = 0.76;
const horizontalWidth = Math.ceil(148 + Number(wordWidth) * wordScale + 12);
const horizontal = '<svg xmlns="http://www.w3.org/2000/svg" width="' + horizontalWidth + '" height="128" viewBox="0 0 ' + horizontalWidth + ' 128" role="img" aria-label="Glide-Dock" color="#16243A">\n'
  + '  <title>Glide-Dock</title>\n'
  + '  <style>@media (prefers-color-scheme:dark) { :root { color:#E7EEFB; } }</style>\n'
  + inner(mark) + '\n  <g transform="translate(148 ' + ((128 - Number(wordHeight) * wordScale) / 2) + ') scale(' + wordScale + ')">\n'
  + inner(wordmark) + '\n  </g>\n</svg>\n';
const variants = {
  FullColor: mark,
  Light: mark.replaceAll('#13B6B3', '#078C91').replaceAll('#2A88F4', '#216DE0').replaceAll('#5860E8', '#4C43CE'),
  Dark: mark.replaceAll('#13B6B3', '#51E4CD').replaceAll('#2A88F4', '#6AB5FF').replaceAll('#5860E8', '#9B99FF'),
  Mono: mark.replace(/<defs>[\s\S]*?<\/defs>/, '').replace('role="img"', 'color="#26334D" role="img"')
    .replace('fill="url(#glide-gradient)"', 'fill="currentColor"'),
};
const sizes = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256, 512, 1024];
const icoSizes = sizes.filter(size => size <= 256);
const dataUrl = svg => 'data:image/svg+xml;base64,' + Buffer.from(svg).toString('base64');
async function save(relative, bytes) {
  const destination = path.join(brand, relative);
  await mkdir(path.dirname(destination), { recursive: true });
  await writeFile(destination, bytes);
}
function encodeIco(frames) {
  const directory = Buffer.alloc(6 + frames.length * 16);
  directory.writeUInt16LE(1, 2);
  directory.writeUInt16LE(frames.length, 4);
  let offset = directory.length;
  frames.forEach(({ size, bytes }, index) => {
    const entry = 6 + index * 16;
    directory[entry] = directory[entry + 1] = size === 256 ? 0 : size;
    directory.writeUInt16LE(1, entry + 4);
    directory.writeUInt16LE(32, entry + 6);
    directory.writeUInt32LE(bytes.length, entry + 8);
    directory.writeUInt32LE(offset, entry + 12);
    offset += bytes.length;
  });
  return Buffer.concat([directory, ...frames.map(frame => frame.bytes)]);
}

let browserPath = values.browser ?? process.env.GLIDE_BRAND_BROWSER;
if (!browserPath && process.platform === 'win32') {
  try { await access(chromium.executablePath()); }
  catch {
    for (const directory of [process.env['ProgramFiles(x86)'], process.env.ProgramFiles, process.env.LOCALAPPDATA].filter(Boolean)) {
      const candidate = path.join(directory, 'Microsoft', 'Edge', 'Application', 'msedge.exe');
      try { await access(candidate); browserPath = candidate; break; } catch { /* Try the next installation. */ }
    }
  }
}
const browser = await chromium.launch({ headless: true, ...(browserPath ? { executablePath: browserPath } : {}) });
try {
  const page = await browser.newPage({ viewport: { width: 1024, height: 768 }, colorScheme: 'light' });
  async function raster(svg, width, height = width) {
    const rendered = await page.evaluate(async ({ svg, width, height }) => {
      const image = new Image();
      image.src = 'data:image/svg+xml;base64,' + btoa(unescape(encodeURIComponent(svg)));
      await image.decode();
      const canvas = document.createElement('canvas');
      canvas.width = width; canvas.height = height;
      const context = canvas.getContext('2d');
      context.drawImage(image, 0, 0, width, height);
      const pixels = context.getImageData(0, 0, width, height).data;
      const corners = [0, width - 1, width * (height - 1), width * height - 1];
      const transparentCorners = corners.every(index => pixels[index * 4 + 3] === 0);
      let opaqueBlack = false;
      for (let index = 0; index < pixels.length; index += 4) {
        // Ignore sub-visible coverage where 8-bit premultiplied colors round to zero.
        if (pixels[index + 3] >= 16 && Math.max(pixels[index], pixels[index + 1], pixels[index + 2]) < 12) opaqueBlack = true;
      }
      return { base64: canvas.toDataURL('image/png').split(',')[1], transparentCorners, opaqueBlack };
    }, { svg, width, height });
    if (!rendered.transparentCorners || rendered.opaqueBlack) throw new Error('An export contains a background or black edge.');
    const bytes = Buffer.from(rendered.base64, 'base64');
    if (bytes.readUInt32BE(16) !== width || bytes.readUInt32BE(20) !== height) throw new Error('Unexpected PNG dimensions.');
    return bytes;
  }
  for (const [variant, svg] of Object.entries(variants)) {
    const suffix = variant === 'FullColor' ? '' : '-' + variant;
    await save('01-Logo/Glide-Dock-Logo-Symbol' + suffix + '.svg', svg);
    const frames = [];
    for (const size of sizes) {
      const bytes = await raster(svg, size);
      await save('02-App-Icons/' + variant + '/Glide-Dock-' + variant + '-' + size + 'x' + size + '.png', bytes);
      if (icoSizes.includes(size)) frames.push({ size, bytes });
    }
    await save('03-Windows-ICO/Glide-Dock-' + variant + '.ico', encodeIco(frames));
  }
  await save('01-Logo/Glide-Dock-Logo-Horizontal.svg', horizontal);
  await save('01-Logo/Glide-Dock-Logo-Horizontal.png', await raster(horizontal, horizontalWidth * 2, 256));
  await copyFile(path.join(brand, '03-Windows-ICO', 'Glide-Dock-FullColor.ico'), path.join(root, 'settings-app', 'Assets', 'Glide-Dock-FullColor.ico'));
  await copyFile(path.join(brand, '02-App-Icons', 'FullColor', 'Glide-Dock-FullColor-128x128.png'), path.join(root, 'settings-app', 'Assets', 'Glide-Dock-FullColor-128.png'));

  await page.setViewportSize({ width: 1000, height: 600 });
  await page.setContent('<!doctype html><html lang="zh-CN"><meta charset="utf-8"><style>'
    + '*{box-sizing:border-box}body{margin:0;padding:32px;background:#eef2f7;color:#1c2a40;font:14px "Segoe UI",sans-serif}'
    + 'header{display:flex;justify-content:space-between;align-items:center;margin-bottom:24px}header strong{font-size:22px;font-weight:600;letter-spacing:-.5px}header span{color:#66768c;font-size:12px;letter-spacing:1px}'
    + 'main{display:grid;grid-template-columns:1fr 1fr;gap:18px}.panel{height:346px;border-radius:22px;padding:24px 28px;display:flex;flex-direction:column;align-items:center;justify-content:space-between}.light{background:#fff}.dark{background:#141d2d;color:#c4d3eb}'
    + '.panel>img{width:224px;height:224px}.label{align-self:flex-start;font-size:12px;letter-spacing:1.5px;opacity:.6}.samples{display:flex;align-items:center;justify-content:center;gap:24px;height:52px}'
    + 'footer{margin-top:18px;padding:20px 24px;background:white;border-radius:18px;display:flex;align-items:center;justify-content:space-between}footer>img{width:302px;height:auto}footer span{color:#78879c;font-size:12px}'
    + '</style><header><strong>Glide-Dock</strong><span>G MARK / VECTOR IDENTITY</span></header><main>'
    + ['light', 'dark'].map(theme => '<section class="panel ' + theme + '"><span class="label">' + (theme === 'light' ? 'LIGHT' : 'DARK') + '</span><img src="' + dataUrl(mark) + '" alt="Glide-Dock G"><div class="samples">'
      + [16, 20, 24, 32, 48].map(size => '<img width="' + size + '" height="' + size + '" src="' + dataUrl(mark) + '" alt="' + size + ' pixels">').join('') + '</div></section>').join('')
    + '</main><footer><img src="' + dataUrl(horizontal) + '" alt="Glide-Dock"><span>SVG · Transparent · 16–1024 px</span></footer></html>');
  await page.locator('img').evaluateAll(images => Promise.all(images.map(image => image.decode())));
  await page.screenshot({ path: path.join(brand, '04-Showcase', 'Glide-Dock-Brand-Preview.png') });
  console.log('Generated vector logos, 48 transparent PNGs, four 10-frame ICOs, and the brand preview.');
} finally {
  await browser.close();
}
