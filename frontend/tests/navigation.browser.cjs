// PLAYWRIGHT_MODULE=/path/to/playwright NAV_TEST_URL=http://127.0.0.1:5193 node frontend/tests/navigation.browser.cjs
// APIs are mocked; no real account data or writes are used.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
(async () => {
  const browser = await chromium.launch({ headless: true, args: ['--no-sandbox'] });
  try {
    const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
    page.setDefaultTimeout(6000);
    const errors = [];
    page.on('pageerror', e => errors.push(e.message));
    await page.route('**/api/**', async route => {
      assert.equal(route.request().method(), 'GET', 'navigation must not write business data');
      const path = new URL(route.request().url()).pathname;
      let data = [];
      if (path === '/api/settings') data = { log_level: 'info', proxy: null, lightpanda: {}, browserless: {} };
      if (path === '/api/media/settings') data = { tmdb_token_configured: false, tmdb_language: 'zh-CN', scan_interval_mins: 30, max_search_queries: 3, search_concurrency: 2 };
      if (path === '/api/media/openlist/settings') return route.fulfill({ status: 503, json: { error: '模拟暂不可用' } });
      await route.fulfill({ json: data });
    });
    const url = process.env.NAV_TEST_URL || 'http://127.0.0.1:5193';
    const tab = name => page.getByRole('tab', { name, exact: true });
    const selected = async name => { await page.waitForFunction(name => [...document.querySelectorAll('[role=tab]')].some(e => e.textContent.trim() === name && e.getAttribute('aria-selected') === 'true'), name); };
    const capture = async name => { if (process.env.NAV_SCREENSHOTS) { fs.mkdirSync(process.env.NAV_SCREENSHOTS, { recursive: true }); await page.screenshot({ path: `${process.env.NAV_SCREENSHOTS}/${name}.png`, fullPage: true }); } };
    await page.goto(`${url}/#/media`);
    await page.getByRole('button', { name: '配置质量配置', exact: true }).click();
    await selected('质量配置');
    assert.match(page.url(), /mode=settings&section=quality/);
    assert(await page.getByRole('heading', { name: '质量配置', exact: true }).isVisible());
    assert.equal(await page.getByLabel('TMDB API Key 或 Read Token', { exact: true }).isVisible(), false);
    await page.reload(); await selected('质量配置');
    await tab('媒体设置').click(); await selected('媒体设置');
    await page.goBack(); await selected('质量配置');
    await tab('质量配置').focus(); await page.keyboard.press('ArrowRight'); await selected('OpenList');
    await page.getByText('OpenList 设置暂不可用', { exact: true }).waitFor();
    await tab('资源搜索').click(); await selected('资源搜索');
    await page.locator('#resource-query').fill('测试影视 S02');
    await page.reload(); await selected('资源搜索');
    assert.equal(await page.locator('#resource-query').inputValue(), '测试影视 S02');
    await page.getByRole('button', { name: '系统设置', exact: true }).click();
    await page.getByRole('heading', { name: '系统设置', exact: true }).first().waitFor();
    await page.goBack(); await selected('资源搜索');
    assert.equal(await page.locator('#resource-query').inputValue(), '测试影视 S02');
    await page.getByRole('button', { name: '系统设置', exact: true }).click();
    await page.getByRole('button', { name: '自动追剧', exact: true }).click();
    await selected('资源搜索');
    await page.getByRole('button', { name: '资源与订阅', exact: true }).click();
    assert(await page.getByRole('button', { name: '资源与订阅 当前：自动追剧' }).isVisible());
    await page.getByRole('button', { name: '资源与订阅 当前：自动追剧' }).click();
    assert.equal(await page.getByRole('button', { name: '系统总览', exact: true }).count(), 1);
    await capture('desktop');
    await page.getByRole('button', { name: '自动签到', exact: true }).click();
    await page.locator('#sign-in-records-tab').click(); await page.waitForFunction(() => document.getElementById('sign-in-records-tab')?.getAttribute('aria-selected') === 'true');
    assert.match(page.url(), /view=records/);
    await page.reload(); await page.waitForFunction(() => document.getElementById('sign-in-records-tab')?.getAttribute('aria-selected') === 'true');
    await page.goto(`${url}/#/media?mode=settings&section=quality`); await selected('质量配置');
    await page.setViewportSize({ width: 390, height: 844 });
    for (const label of ['追剧', '统计', '刷流', '站点']) {
      const item = page.locator('.mobile-dock').getByRole('button', { name: label, exact: true });
      assert.equal((await item.innerText()).trim(), label);
    }
    assert(await page.getByRole('button', { name: '实时日志', exact: true }).isVisible());
    await capture('mobile');
    const trigger = page.getByRole('button', { name: '打开全部菜单', exact: true });
    await trigger.focus(); await page.keyboard.press('Enter');
    const dialog = page.getByRole('dialog', { name: '主菜单' }); await dialog.waitFor();
    await page.waitForFunction(() => !!document.activeElement.closest('[role=dialog]'));
    assert.equal(await page.locator('main').getAttribute('inert'), '');
    const focusable = dialog.locator('button:visible, a:visible');
    await focusable.last().focus(); await page.keyboard.press('Tab');
    assert(await focusable.first().evaluate(e => e === document.activeElement));
    await page.keyboard.press('Shift+Tab');
    assert(await focusable.last().evaluate(e => e === document.activeElement));
    await capture('drawer');
    await page.keyboard.press('Escape'); await dialog.waitFor({ state: 'hidden' });
    assert(await trigger.evaluate(e => e === document.activeElement));
    assert.equal(await page.locator('main').getAttribute('inert'), null);
    await trigger.click();
    await page.getByRole('dialog').getByRole('button', { name: '系统设置', exact: true }).click();
    await dialog.waitFor({ state: 'hidden' });
    assert.match(page.url(), /#\/system-settings$/);
    await trigger.click(); await page.setViewportSize({ width: 1440, height: 900 });
    await page.locator('#mobile-navigation').waitFor({ state: 'detached' });
    assert.equal(await page.locator('main').getAttribute('inert'), null);
    await page.setViewportSize({ width: 320, height: 640 });
    assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
    // Runtime SELF_USE gates desktop, mobile, and direct hash navigation. Fail closed.
    for (const flag of [true, false, undefined, 'true', 'unavailable']) {
      const gated = await browser.newPage({ viewport: { width: 1440, height: 900 } });
      const transferRequests = [];
      gated.on('pageerror', e => errors.push(e.message));
      await gated.route('**/api/**', async route => {
        const path = new URL(route.request().url()).pathname;
        if (path === '/api/features') {
          if (flag === 'unavailable') return route.fulfill({ status: 503, json: {} });
          return route.fulfill({ json: { self_use: flag } });
        }
        if (path === '/api/settings') return route.fulfill({ json: { log_level: 'info', proxy: null, lightpanda: {}, browserless: {} } });
        if (path.includes('/openlist/') || path.endsWith('/torrents')) transferRequests.push(path);
        return route.fulfill({ status: 503, json: { error: '模拟服务不可用' } });
      });
      await gated.goto(`${url}/#/system-settings`);
      await gated.getByRole('heading', { name: '系统设置', exact: true }).first().waitFor();
      assert.equal(await gated.getByRole('button', { name: '种子转移', exact: true }).count(), flag === true ? 1 : 0);
      await gated.setViewportSize({ width: 390, height: 844 });
      await gated.getByRole('button', { name: '打开全部菜单', exact: true }).click();
      assert.equal(await gated.getByRole('dialog', { name: '主菜单' }).getByRole('button', { name: '种子转移', exact: true }).count(), flag === true ? 1 : 0);
      await gated.goto(`${url}/#/torrent-transfer`);
      if (flag === true) {
        await gated.getByRole('heading', { name: '种子转移', exact: true }).first().waitFor();
        assert.match(gated.url(), /#\/torrent-transfer$/);
      } else {
        await gated.waitForURL('**/#/');
        assert.equal(await gated.getByRole('heading', { name: '种子转移', exact: true }).count(), 0);
        assert.deepEqual(transferRequests, [], 'disabled transfer route must never mount or request APIs');
      }
      await gated.close();
    }
    assert.deepEqual(errors, []);
    console.log('Navigation checks passed: history, refresh, query drafts, settings targeting, tab keyboard navigation, mobile labels, modal focus, resize, overflow, SELF_USE on/off/missing/invalid/unavailable.');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exit(1); });
