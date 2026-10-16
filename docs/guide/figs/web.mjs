// The guide's screenshot of cinefit's web page: the real page
// (crates/cinefit/assets/web/index.html) in a phone-sized browser, its API
// answered from the sample state the web_state_preview test writes.
//
// From the repository root, with Playwright installed somewhere on the
// module path (npm i --no-save playwright):
//
//   CINEFIT_OSD_PREVIEW=/tmp/osd cargo test -p cinefit web_state_preview
//   node docs/guide/figs/web.mjs /tmp/osd
import { chromium } from 'playwright';
import { readFileSync } from 'node:fs';

const dir = process.argv[2];
const page = readFileSync('crates/cinefit/assets/web/index.html', 'utf8');
const state = readFileSync(`${dir}/web-state.json`, 'utf8');
const logs = JSON.stringify([
  { name: 'play-2026-10-15-201502.log', size: 48211 },
  { name: 'play-2026-10-14-190833.log', size: 61877 },
  { name: 'play-2026-10-13-213010.log', size: 39004 },
]);
const json = body => ({ status: 200, contentType: 'application/json', body });

const browser = await chromium.launch();
const p = await browser.newPage({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 3 });
await p.route('http://cinefit.local/**', route => {
  const path = new URL(route.request().url()).pathname;
  if (path === '/') return route.fulfill({ status: 200, contentType: 'text/html', body: page });
  if (path === '/api/state') return route.fulfill(json(state));
  if (path === '/api/logs') return route.fulfill(json(logs));
  if (path === '/api/atv') return route.fulfill(json('{"paired":true}'));
  return route.fulfill({ status: 404, body: '' });
});
await p.goto('http://cinefit.local/', { waitUntil: 'networkidle' });
await p.waitForSelector('#pages details');
// The status, as the page opens: a phone's screen.
await p.screenshot({ path: 'docs/guide/assets/images/web-status.png' });
// The settings, with the Picture page open.
await p.evaluate(() => document.querySelector('#pages details').open = true);
// The settings, with the Picture page open: its first rows, a phone's
// screen high.
const box = await (await p.$('#settings')).boundingBox();
await p.setViewportSize({ width: 390, height: Math.ceil(box.y + 760) });
await p.screenshot({ path: 'docs/guide/assets/images/web-settings.png', clip: { x: 0, y: box.y - 12, width: 390, height: 772 } });
await browser.close();
