// PREVIEW HARNESS ONLY (dev-only; `pnpm preview:ui:shots`). Screenshots every preview scenario at several window
// sizes with the system Chrome, driven by playwright-core (no new dependency: it is resolved from node_modules or,
// failing that, from the npx cache of @playwright/mcp).
//
//   node preview/capture.mjs [--out <dir>] [--sizes 1920x1080,1366x768,2560x1440] [--only <substring>]
//                            [--headed] [--base http://localhost:1431] [--no-server]
//
// Unless --no-server is given it starts the preview dev server itself (vite createServer on port 1431) and stops it
// when done. For every shot it waits for <html data-preview-ready="true">, then checks every visible <canvas> for
// blank content (a single flat colour) by sampling its pixels from a screenshot, and records console errors.
// A summary (report.json) is written next to the PNGs. Output defaults to the D: build folder, never C: or the repo.
import { existsSync, mkdirSync, readdirSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { homedir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const desktopDir = fileURLToPath(new URL('..', import.meta.url));
const DEFAULT_OUT = 'D:\\RaceEngineeringData\\build\\session-62bdcac0\\ui\\shots';
const DEFAULT_SIZES = '1920x1080,1366x768,2560x1440';

function parseArgs(argv) {
  const args = { out: DEFAULT_OUT, sizes: DEFAULT_SIZES, only: null, headed: false, base: null, server: true };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const next = () => {
      const v = argv[++i];
      if (v === undefined) throw new Error(`${a} needs a value`);
      return v;
    };
    if (a === '--out') args.out = next();
    else if (a === '--sizes') args.sizes = next();
    else if (a === '--only') args.only = next();
    else if (a === '--headed') args.headed = true;
    else if (a === '--base') args.base = next();
    else if (a === '--no-server') args.server = false;
    else throw new Error(`unknown argument ${a}`);
  }
  if (/^c:/i.test(resolve(args.out))) throw new Error('refusing to write screenshots to C: (use a D: path)');
  return args;
}

async function loadPlaywright() {
  const require = createRequire(join(desktopDir, 'package.json'));
  try {
    return await import(pathToFileURL(require.resolve('playwright-core')).href);
  } catch {
    // Fall back to the npx cache (e.g. the copy @playwright/mcp brought in).
  }
  const roots = [
    process.env.npm_config_cache && join(process.env.npm_config_cache, '_npx'),
    process.env.LOCALAPPDATA && join(process.env.LOCALAPPDATA, 'npm-cache', '_npx'),
    join(homedir(), '.npm', '_npx'),
  ].filter(Boolean);
  for (const root of roots) {
    if (!existsSync(root)) continue;
    for (const entry of readdirSync(root)) {
      const candidate = join(root, entry, 'node_modules', 'playwright-core', 'index.mjs');
      if (existsSync(candidate)) return await import(pathToFileURL(candidate).href);
    }
  }
  throw new Error('playwright-core not found (install it or run `npx @playwright/mcp --help` once to cache it)');
}

async function startServer() {
  const require = createRequire(join(desktopDir, 'package.json'));
  const vite = await import(pathToFileURL(require.resolve('vite')).href);
  const server = await vite.createServer({
    configFile: join(desktopDir, 'preview', 'vite.config.ts'),
    logLevel: 'warn',
  });
  await server.listen();
  return server;
}

/** Counts distinct colours (quantised) in a PNG screenshot of one canvas, inside the page. */
async function canvasColourCount(page, png) {
  const dataUrl = `data:image/png;base64,${png.toString('base64')}`;
  return page.evaluate(async (url) => {
    const img = new Image();
    img.src = url;
    await img.decode();
    const c = document.createElement('canvas');
    c.width = img.width;
    c.height = img.height;
    const ctx = c.getContext('2d');
    ctx.drawImage(img, 0, 0);
    const { data } = ctx.getImageData(0, 0, c.width, c.height);
    const colours = new Set();
    for (let i = 0; i < data.length; i += 4 * 7) {
      colours.add(((data[i] >> 3) << 10) | ((data[i + 1] >> 3) << 5) | (data[i + 2] >> 3));
      if (colours.size > 64) break;
    }
    return colours.size;
  }, dataUrl);
}

async function checkCanvases(page) {
  const handles = await page.$$('canvas');
  const blank = [];
  let checked = 0;
  for (const handle of handles) {
    const box = await handle.boundingBox();
    if (box === null || box.width < 40 || box.height < 40) continue;
    const visible = await handle.isVisible();
    if (!visible) continue;
    // Only the part inside the viewport (the shot is of the viewport; scrolled-away canvases are not judged).
    const vp = page.viewportSize();
    const x0 = Math.max(0, box.x);
    const y0 = Math.max(0, box.y);
    const x1 = Math.min(vp.width, box.x + box.width);
    const y1 = Math.min(vp.height, box.y + box.height);
    if (x1 - x0 < 40 || y1 - y0 < 40) continue;
    checked++;
    const png = await page.screenshot({ clip: { x: x0, y: y0, width: x1 - x0, height: y1 - y0 } });
    const colours = await canvasColourCount(page, png);
    if (colours <= 2) {
      const label = await handle.evaluate((el) => {
        const owner = el.closest('[data-testid], [aria-label], section, .panel');
        return (
          owner?.getAttribute('data-testid') ??
          owner?.getAttribute('aria-label') ??
          owner?.className ??
          el.className ??
          'canvas'
        );
      });
      blank.push({ label, width: Math.round(box.width), height: Math.round(box.height), colours });
    }
  }
  return { checked, blank };
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const sizes = args.sizes.split(',').map((s) => {
    const [w, h] = s.split('x').map(Number);
    if (!(w > 0 && h > 0)) throw new Error(`bad size ${s}`);
    return { w, h };
  });
  const { chromium } = await loadPlaywright();
  const server = args.server && args.base === null ? await startServer() : null;
  const base = args.base ?? 'http://localhost:1431';
  const browser = await chromium.launch({
    channel: 'chrome',
    headless: !args.headed,
    // WebGL in headless Chrome: software GL (SwiftShader) so graph canvases render.
    args: ['--use-angle=swiftshader', '--enable-unsafe-swiftshader', '--force-color-profile=srgb', '--hide-scrollbars'],
  });
  const report = { base, startedAt: new Date().toISOString(), shots: [] };
  try {
    // Scenario ids from the index page.
    const indexPage = await browser.newPage();
    await indexPage.goto(`${base}/`, { waitUntil: 'load' });
    await indexPage.waitForSelector('html[data-preview-ready="true"]', { state: 'attached', timeout: 60_000 });
    const ids = await indexPage.$$eval('a[href^="?scenario="]', (as) =>
      as.map((a) => new URLSearchParams(a.getAttribute('href') ?? '').get('scenario')).filter(Boolean),
    );
    await indexPage.close();
    const selected = ['index', ...ids].filter((id) => args.only === null || id.includes(args.only));
    console.log(`${selected.length} scenario(s) x ${sizes.length} size(s) -> ${args.out}`);

    for (const { w, h } of sizes) {
      const dir = join(args.out, `${w}x${h}`);
      mkdirSync(dir, { recursive: true });
      for (const id of selected) {
        const context = await browser.newContext({ viewport: { width: w, height: h }, deviceScaleFactor: 1, colorScheme: 'dark' });
        const page = await context.newPage();
        const errors = [];
        page.on('console', (msg) => {
          if (msg.type() === 'error') errors.push(msg.text());
        });
        page.on('pageerror', (err) => errors.push(`pageerror: ${err.message}`));
        const url = id === 'index' ? `${base}/` : `${base}/?scenario=${id}`;
        const started = Date.now();
        let stepError = null;
        try {
          await page.goto(url, { waitUntil: 'load' });
          await page.waitForSelector('html[data-preview-ready="true"]', { state: 'attached', timeout: 60_000 });
          stepError = await page.evaluate(() => document.documentElement.dataset.previewError ?? null);
        } catch (error) {
          stepError = `not ready: ${error instanceof Error ? error.message : String(error)}`;
        }
        const file = join(dir, `${id}.png`);
        await page.screenshot({ path: file });
        const canvases = id === 'index' ? { checked: 0, blank: [] } : await checkCanvases(page);
        const overflow = await page.evaluate(() => {
          const d = document.documentElement;
          return { x: d.scrollWidth > d.clientWidth, y: d.scrollHeight > d.clientHeight };
        });
        const shot = { id, size: `${w}x${h}`, file, ms: Date.now() - started, stepError, errors, canvases, overflow };
        report.shots.push(shot);
        const flags = [
          stepError ? `STEP ERROR: ${stepError}` : '',
          canvases.blank.length ? `BLANK CANVAS x${canvases.blank.length}` : '',
          errors.length ? `${errors.length} console error(s)` : '',
          overflow.x || overflow.y ? `page overflow ${overflow.x ? 'x' : ''}${overflow.y ? 'y' : ''}` : '',
        ].filter(Boolean);
        console.log(`${w}x${h} ${id} (${canvases.checked} canvas)${flags.length ? '  ' + flags.join('; ') : ''}`);
        await context.close();
      }
    }
  } finally {
    await browser.close();
    if (server !== null) await server.close();
    mkdirSync(args.out, { recursive: true });
    writeFileSync(join(args.out, 'report.json'), JSON.stringify(report, null, 2));
  }
  const bad = report.shots.filter((s) => s.stepError || s.canvases.blank.length > 0);
  console.log(`done: ${report.shots.length} shot(s), ${bad.length} with step errors or blank canvases`);
  if (bad.length > 0) process.exitCode = 1;
}

main().catch((error) => {
  console.error(error);
  process.exit(2);
});
