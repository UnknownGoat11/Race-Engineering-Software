// PREVIEW HARNESS ONLY (dev-only; `pnpm preview:ui:shots`). Screenshots every preview scenario at several window
// sizes with the system Chrome, driven by playwright-core (no new dependency: it is resolved from node_modules or,
// failing that, from the npx cache of @playwright/mcp).
//
//   node preview/capture.mjs [--out <dir>] [--sizes 1920x1080,1366x768,2560x1440] [--only <substring>]
//                            [--diff <baselineDir>] [--fail-on-clip] [--headed] [--base http://localhost:1431]
//                            [--no-server]
//
// Unless --no-server is given it starts the preview dev server itself (vite createServer on port 1431) and stops it
// when done. For every shot it waits for <html data-preview-ready="true"> (the page has frozen the fake backend at
// the scenario's planned capture time and held its clocks, so the same scenario renders the same pixels on every
// run), takes the screenshot with CSS animations disabled, then checks:
//   - canvases: every visible <canvas> for blank content (a single flat colour), sampled from a screenshot;
//   - clipping: every visible element with `overflow: hidden | clip` (either axis) and every `[data-clip-check]`
//     element whose content is larger than its box (`scrollWidth > clientWidth + 1` or
//     `scrollHeight > clientHeight + 1`), reported as `clipped: [{ selector, text, axis, ... }]`;
//   - page overflow (document scrollbars) and console errors;
//   - with --diff <baselineDir>: a per-pixel diff against <baselineDir>/<size>/<id>.png, computed in the browser
//     like the canvas check. Writes <id>.diff.png (changed pixels red over a dimmed copy of the new shot) and
//     `diff: { diffPct, changedPixels, ... }` into report.json.
// A summary (report.json) is written next to the PNGs. Output defaults to the D: build folder, never C: or the repo.
// Exit code: 1 when a shot has a step error or a blank canvas, or (with --fail-on-clip) a clipped element; 2 on a
// harness failure.
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { homedir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const desktopDir = fileURLToPath(new URL('..', import.meta.url));
const repoDir = resolve(desktopDir, '..', '..');
const DEFAULT_OUT = 'D:\\RaceEngineeringData\\build\\session-62bdcac0\\ui\\shots';
const DEFAULT_SIZES = '1920x1080,1366x768,2560x1440';
/** A pixel counts as changed when any colour channel differs by more than this (0-255). */
const DIFF_CHANNEL_TOLERANCE = 24;

function parseArgs(argv) {
  const args = {
    out: DEFAULT_OUT,
    sizes: DEFAULT_SIZES,
    only: null,
    headed: false,
    base: null,
    server: true,
    diff: null,
    failOnClip: false,
  };
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
    else if (a === '--diff') args.diff = next();
    else if (a === '--fail-on-clip') args.failOnClip = true;
    else throw new Error(`unknown argument ${a}`);
  }
  const out = resolve(args.out);
  if (/^c:/i.test(out)) throw new Error('refusing to write screenshots to C: (use a D: path)');
  if (out.toLowerCase().startsWith(repoDir.toLowerCase())) throw new Error('refusing to write screenshots into the repo');
  if (args.diff !== null) {
    args.diff = resolve(args.diff);
    if (!existsSync(args.diff)) throw new Error(`--diff baseline directory not found: ${args.diff}`);
    if (args.diff.toLowerCase() === out.toLowerCase()) throw new Error('--diff baseline must differ from --out');
  }
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
    const png = await page.screenshot({ clip: { x: x0, y: y0, width: x1 - x0, height: y1 - y0 }, animations: 'disabled' });
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

/**
 * Elements whose content does not fit their box while their overflow is hidden / clipped (or that opt in with
 * `data-clip-check`). Scroll containers (`auto` / `scroll`) are not clipping. Elements outside the viewport,
 * invisible or at most 1 px in either dimension (visually-hidden text) are skipped. The preview tag is ignored.
 */
async function checkClipping(page) {
  return page.evaluate(() => {
    const out = [];
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    const clips = (v) => v === 'hidden' || v === 'clip';
    const describe = (el) => {
      const parts = [];
      let node = el;
      for (let depth = 0; node && node.nodeType === 1 && depth < 4; depth++) {
        let part = node.tagName.toLowerCase();
        const testId = node.getAttribute('data-testid');
        if (testId) {
          parts.unshift(`${part}[data-testid="${testId}"]`);
          break;
        }
        if (node.id) {
          parts.unshift(`${part}#${node.id}`);
          break;
        }
        const cls = [...node.classList].slice(0, 2);
        if (cls.length) part += `.${cls.join('.')}`;
        const parent = node.parentElement;
        if (parent) {
          const same = [...parent.children].filter((c) => c.tagName === node.tagName);
          if (same.length > 1) part += `:nth-of-type(${same.indexOf(node) + 1})`;
        }
        parts.unshift(part);
        node = parent;
      }
      return parts.join(' > ');
    };
    for (const el of document.querySelectorAll('body *')) {
      if (el.closest('.preview-tag')) continue;
      const style = getComputedStyle(el);
      const optIn = el.hasAttribute('data-clip-check');
      const clipX = clips(style.overflowX);
      const clipY = clips(style.overflowY);
      if (!optIn && !clipX && !clipY) continue;
      if (style.display === 'none' || style.visibility === 'hidden') continue;
      const rect = el.getBoundingClientRect();
      // Zero / 1 px boxes are skipped: that is the visually-hidden (screen-reader only) pattern, clipped on purpose.
      if (rect.width <= 1 || rect.height <= 1) continue;
      if (rect.right <= 0 || rect.bottom <= 0 || rect.left >= vw || rect.top >= vh) continue;
      const overX = el.scrollWidth > el.clientWidth + 1;
      const overY = el.scrollHeight > el.clientHeight + 1;
      const x = overX && (clipX || optIn);
      const y = overY && (clipY || optIn);
      if (!x && !y) continue;
      const text = (el.innerText ?? el.textContent ?? '').replace(/\s+/g, ' ').trim().slice(0, 80);
      out.push({
        selector: describe(el),
        text,
        axis: x && y ? 'xy' : x ? 'x' : 'y',
        box: { w: el.clientWidth, h: el.clientHeight },
        content: { w: el.scrollWidth, h: el.scrollHeight },
        ellipsis: style.textOverflow === 'ellipsis',
        optIn,
      });
    }
    return out;
  });
}

/**
 * Per-pixel diff of two PNGs (Buffers), computed in `page`. Returns the stats and the diff image as a PNG
 * Buffer: changed pixels red, unchanged pixels a dimmed grey copy of the new shot.
 */
async function pixelDiff(page, currentPng, baselinePng) {
  const result = await page.evaluate(
    async ({ cur, base, tol }) => {
      const load = async (b64) => {
        const img = new Image();
        img.src = `data:image/png;base64,${b64}`;
        await img.decode();
        return img;
      };
      const [a, b] = await Promise.all([load(cur), load(base)]);
      const w = Math.max(a.width, b.width);
      const h = Math.max(a.height, b.height);
      const read = (img) => {
        const c = document.createElement('canvas');
        c.width = w;
        c.height = h;
        const ctx = c.getContext('2d');
        ctx.drawImage(img, 0, 0);
        return ctx.getImageData(0, 0, w, h).data;
      };
      const da = read(a);
      const db = read(b);
      const out = document.createElement('canvas');
      out.width = w;
      out.height = h;
      const octx = out.getContext('2d');
      const diff = octx.createImageData(w, h);
      const dd = diff.data;
      let changed = 0;
      let minX = w;
      let minY = h;
      let maxX = -1;
      let maxY = -1;
      for (let i = 0, p = 0; i < da.length; i += 4, p++) {
        const x = p % w;
        const y = (p - x) / w;
        const outside = x >= Math.min(a.width, b.width) || y >= Math.min(a.height, b.height);
        const d = Math.max(Math.abs(da[i] - db[i]), Math.abs(da[i + 1] - db[i + 1]), Math.abs(da[i + 2] - db[i + 2]));
        if (outside || d > tol) {
          changed++;
          dd[i] = 255;
          dd[i + 1] = 0;
          dd[i + 2] = 0;
          if (x < minX) minX = x;
          if (y < minY) minY = y;
          if (x > maxX) maxX = x;
          if (y > maxY) maxY = y;
        } else {
          const g = Math.round((da[i] * 0.3 + da[i + 1] * 0.59 + da[i + 2] * 0.11) * 0.35);
          dd[i] = g;
          dd[i + 1] = g;
          dd[i + 2] = g;
        }
        dd[i + 3] = 255;
      }
      octx.putImageData(diff, 0, 0);
      const png = out.toDataURL('image/png').slice('data:image/png;base64,'.length);
      return {
        width: w,
        height: h,
        sizeMismatch: a.width !== b.width || a.height !== b.height,
        changedPixels: changed,
        diffPct: Math.round((changed / (w * h)) * 100 * 1000) / 1000,
        changedBox: maxX < 0 ? null : { x: minX, y: minY, w: maxX - minX + 1, h: maxY - minY + 1 },
        png,
      };
    },
    { cur: currentPng.toString('base64'), base: baselinePng.toString('base64'), tol: DIFF_CHANNEL_TOLERANCE },
  );
  const { png, ...stats } = result;
  return { stats, png: Buffer.from(png, 'base64') };
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
  const report = {
    base,
    startedAt: new Date().toISOString(),
    diffBaseline: args.diff,
    diffChannelTolerance: DIFF_CHANNEL_TOLERANCE,
    shots: [],
  };
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
    // A blank page that computes the pixel diffs (keeps that work out of the scenario pages).
    const diffPage = args.diff === null ? null : await browser.newPage();

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
        let freezeLate = false;
        try {
          await page.goto(url, { waitUntil: 'load' });
          await page.waitForSelector('html[data-preview-ready="true"]', { state: 'attached', timeout: 60_000 });
          const flags = await page.evaluate(() => ({
            error: document.documentElement.dataset.previewError ?? null,
            late: document.documentElement.dataset.previewFreezeLate === 'true',
          }));
          stepError = flags.error;
          freezeLate = flags.late;
        } catch (error) {
          stepError = `not ready: ${error instanceof Error ? error.message : String(error)}`;
        }
        const file = join(dir, `${id}.png`);
        const shotPng = await page.screenshot({ path: file, animations: 'disabled' });
        const canvases = id === 'index' ? { checked: 0, blank: [] } : await checkCanvases(page);
        const overflow = await page.evaluate(() => {
          const d = document.documentElement;
          return { x: d.scrollWidth > d.clientWidth, y: d.scrollHeight > d.clientHeight };
        });
        const clipped = await checkClipping(page);
        const tag = await page.evaluate(() => document.querySelector('.preview-tag')?.textContent ?? null);
        let diff = null;
        if (diffPage !== null) {
          const baseFile = join(args.diff, `${w}x${h}`, `${id}.png`);
          if (!existsSync(baseFile)) {
            diff = { baselineMissing: true };
          } else {
            const { stats, png } = await pixelDiff(diffPage, shotPng, readFileSync(baseFile));
            diff = { baseline: baseFile, ...stats };
            if (stats.changedPixels > 0) {
              diff.file = join(dir, `${id}.diff.png`);
              writeFileSync(diff.file, png);
            }
          }
        }
        const shot = {
          id,
          size: `${w}x${h}`,
          file,
          ms: Date.now() - started,
          stepError,
          freezeLate,
          errors,
          canvases,
          overflow,
          clipped,
          previewTag: tag,
          diff,
        };
        report.shots.push(shot);
        const flags = [
          stepError ? `STEP ERROR: ${stepError}` : '',
          freezeLate ? 'FREEZE LATE (frame not reproducible)' : '',
          id !== 'index' && tag === null ? 'NO PREVIEW TAG' : '',
          canvases.blank.length ? `BLANK CANVAS x${canvases.blank.length}` : '',
          errors.length ? `${errors.length} console error(s)` : '',
          overflow.x || overflow.y ? `page overflow ${overflow.x ? 'x' : ''}${overflow.y ? 'y' : ''}` : '',
          clipped.length ? `clipped x${clipped.length}` : '',
          diff?.baselineMissing ? 'no baseline' : diff ? `diff ${diff.diffPct}%` : '',
        ].filter(Boolean);
        console.log(`${w}x${h} ${id} (${canvases.checked} canvas)${flags.length ? '  ' + flags.join('; ') : ''}`);
        await context.close();
      }
    }
    if (diffPage !== null) await diffPage.close();
  } finally {
    await browser.close();
    if (server !== null) await server.close();
    const shots = report.shots;
    report.summary = {
      shots: shots.length,
      stepErrors: shots.filter((s) => s.stepError).length,
      freezeLate: shots.filter((s) => s.freezeLate).length,
      missingPreviewTag: shots.filter((s) => s.id !== 'index' && s.previewTag === null).length,
      blankCanvases: shots.reduce((n, s) => n + s.canvases.blank.length, 0),
      consoleErrors: shots.reduce((n, s) => n + s.errors.length, 0),
      pageOverflow: shots.filter((s) => s.overflow.x || s.overflow.y).length,
      clippedElements: shots.reduce((n, s) => n + s.clipped.length, 0),
      shotsWithClipping: shots.filter((s) => s.clipped.length > 0).length,
      diff:
        args.diff === null
          ? null
          : {
              compared: shots.filter((s) => s.diff && !s.diff.baselineMissing).length,
              baselineMissing: shots.filter((s) => s.diff?.baselineMissing).length,
              over0_5pct: shots.filter((s) => (s.diff?.diffPct ?? 0) > 0.5).map((s) => `${s.size}/${s.id}`),
              changed: shots.filter((s) => (s.diff?.changedPixels ?? 0) > 0).length,
            },
    };
    mkdirSync(args.out, { recursive: true });
    writeFileSync(join(args.out, 'report.json'), JSON.stringify(report, null, 2));
  }
  const s = report.summary;
  console.log(
    `done: ${s.shots} shot(s); step errors ${s.stepErrors}, blank canvases ${s.blankCanvases}, console errors ` +
      `${s.consoleErrors}, page overflow ${s.pageOverflow}, clipped elements ${s.clippedElements} in ` +
      `${s.shotsWithClipping} shot(s), freeze late ${s.freezeLate}, missing tag ${s.missingPreviewTag}` +
      (s.diff ? `; diff: ${s.diff.compared} compared, ${s.diff.changed} changed, ${s.diff.over0_5pct.length} > 0.5 %` : ''),
  );
  const bad = report.shots.filter((x) => x.stepError || x.canvases.blank.length > 0);
  if (bad.length > 0 || s.missingPreviewTag > 0) process.exitCode = 1;
  if (args.failOnClip && s.clippedElements > 0) {
    console.log('--fail-on-clip: clipped elements found');
    process.exitCode = 1;
  }
}

main().catch((error) => {
  console.error(error);
  process.exit(2);
});
