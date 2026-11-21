import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';
import path from 'node:path';
import { setupMock } from './mock';
import type { MockOptions } from './mock';

async function open(page: Page, options: MockOptions = {}) {
  await page.addInitScript(setupMock, options);
  await page.goto('/');
  await expect(page.locator('#version')).toHaveText('v0.4.2');
}
async function preferences(page: Page) {
  await expect(page.locator('#primary-button')).toBeEnabled();
  await page.locator('#primary-button').click();
  await expect(page.locator('#setup-page')).toBeVisible();
}
async function confirm(page: Page) {
  await preferences(page);
  await page.locator('#primary-button').click();
  await expect(page.locator('#confirm-page')).toBeVisible();
}
async function layout(page: Page) {
  const geometry = await page.evaluate(() => {
    const action = document.querySelector('#primary-button')!.getBoundingClientRect();
    return { width: innerWidth, height: innerHeight, scrollWidth: document.documentElement.scrollWidth,
      scrollHeight: document.documentElement.scrollHeight, action: { x: action.x, y: action.y, right: action.right, bottom: action.bottom } };
  });
  const divider = await page.locator('.footer-divider').boundingBox();
  expect(divider!.height).toBeLessThanOrEqual(1);
  expect(geometry.scrollWidth).toBeLessThanOrEqual(geometry.width + 1);
  expect(geometry.scrollHeight).toBeLessThanOrEqual(geometry.height + 1);
  expect(geometry.action.x).toBeGreaterThanOrEqual(0);
  expect(geometry.action.y).toBeGreaterThanOrEqual(0);
  expect(geometry.action.right).toBeLessThanOrEqual(geometry.width + 1);
  expect(geometry.action.bottom).toBeLessThanOrEqual(geometry.height + 1);
}

for (const colorScheme of ['light', 'dark'] as const) {
  for (const [width, height, scale] of [[840, 620, 1], [960, 720, 1.25], [640, 480, 1.5], [560, 400, 2]]) {
    test(colorScheme + ' layout ' + width + 'x' + height + ' @' + scale, async ({ browser }) => {
      const context = await browser.newContext({ viewport: { width, height }, deviceScaleFactor: scale, colorScheme });
      const page = await context.newPage();
      const errors: string[] = [];
      page.on('pageerror', error => errors.push(error.message));
      await open(page);
      await layout(page);
      await preferences(page);
      await layout(page);
      await expect(page.locator('[data-completed-step]')).toHaveCount(1);
      await expect(page.getByRole('tab', { selected: true })).toHaveCount(1);
      await expect(page.locator('#step-setup')).toHaveAttribute('aria-selected', 'true');
      expect(await page.locator('body').innerText()).not.toMatch(/[✓✔]/);
      if (width === 840) {
        const overflow = await page.locator('#main-content').evaluate(element => element.scrollHeight - element.clientHeight);
        expect(overflow).toBeLessThanOrEqual(1);
        await page.screenshot({ path: path.resolve('../../preview/' + (colorScheme === 'light' ? '25-installer-fluent-light.png' : '26-installer-fluent-dark.png')), animations: 'disabled' });
      }
      await page.locator('#main-content').evaluate(element => { element.scrollTop = element.scrollHeight; });
      await page.locator('#primary-button').click();
      await expect(page.locator('#confirm-page')).toBeVisible();
      await expect(page.getByRole('tab', { selected: true })).toHaveCount(1);
      await expect(page.locator('#step-confirm')).toHaveAttribute('aria-selected', 'true');
      await expect(page.locator('#main-content')).toHaveJSProperty('scrollTop', 0);
      await layout(page);
      await page.locator('#back-button').click();
      await expect(page.locator('#setup-page')).toBeVisible();
      await expect(page.locator('#main-content')).toHaveJSProperty('scrollTop', 0);
      expect(errors).toEqual([]);
      await context.close();
    });
  }
}

test('caption controls use aligned 16px icons and compact 40x32px hit areas', async ({ page }) => {
  await open(page);
  const geometry = await page.locator('.window-controls button').evaluateAll(buttons => buttons.map(button => {
    const b = button.getBoundingClientRect();
    const i = button.querySelector('svg')!.getBoundingClientRect();
    return { width: b.width, height: b.height, iconWidth: i.width, iconHeight: i.height, iconY: i.y + i.height / 2 };
  }));
  expect(geometry).toHaveLength(3);
  for (const box of geometry) {
    expect(box.width).toBe(40); expect(box.height).toBe(32); expect(box.iconWidth).toBe(16); expect(box.iconHeight).toBe(16);
    expect(box.iconY).toBe(geometry[0].iconY);
  }
  await page.locator('#maximize-button').click();
  await expect(page.locator('#maximize-button')).toHaveAttribute('aria-label', '还原');
  await page.locator('#maximize-button').click();
  await expect(page.locator('#maximize-button')).toHaveAttribute('aria-label', '最大化');
  await page.locator('#minimize-button').click();
  expect(await page.evaluate(() => (window as any).__calls.some((call: any) => call.command === 'window.minimize'))).toBe(true);
});

test('app search and selection preserve choices across wizard navigation', async ({ page }) => {
  await open(page); await preferences(page);
  await page.locator('#apps-toggle').click();
  await page.locator('#app-search').fill('不存在');
  await expect(page.locator('#app-list').getByRole('checkbox')).toHaveCount(0);
  await page.locator('#app-search').fill('示例');
  await page.locator('#select-apps').click();
  await expect(page.locator('#app-list').getByRole('checkbox', { checked: true })).toHaveCount(17);
  await page.locator('#primary-button').click(); await page.locator('#back-button').click();
  await expect(page.locator('#app-list').getByRole('checkbox', { checked: true })).toHaveCount(17);
  await page.locator('#clear-apps').click();
  await expect(page.locator('#app-list').getByRole('checkbox', { checked: true })).toHaveCount(0);
});

test('latest path validation wins and browsing supports Chinese paths', async ({ page }) => {
  await open(page);
  await page.locator('#install-path').fill('C:\\slow-invalid');
  await page.waitForTimeout(220);
  await page.locator('#install-path').fill('D:\\valid');
  await expect(page.locator('#primary-button')).toBeEnabled();
  await page.waitForTimeout(400);
  await expect(page.locator('#primary-button')).toBeEnabled();
  await page.locator('#browse-button').click();
  await expect(page.locator('#install-path')).toHaveValue('D:\\我的应用\\Glide-Dock');
});

test('scope default lookup does not overwrite a newer manually entered path', async ({ page }) => {
  await open(page, { scopeDelay: 350 });
  await page.getByRole('radio', { name: /为所有用户安装/ }).check();
  await page.locator('#install-path').fill('D:\\Manual');
  await page.waitForTimeout(500);
  await expect(page.locator('#install-path')).toHaveValue('D:\\Manual');
});

for (const options of [{ missingPayload: true }, { failedCheck: true }]) {
  test('invalid install payload disables final action ' + JSON.stringify(options), async ({ page }) => {
    await open(page, options); await confirm(page);
    await expect(page.locator('#primary-button')).toBeDisabled();
    expect(await page.evaluate(() => (window as any).__calls.some((call: any) => call.command === 'start_install'))).toBe(false);
  });
}

test('long initialization error remains readable in a scrolling main area', async ({ page }) => {
  await page.setViewportSize({ width: 560, height: 400 });
  await page.addInitScript(setupMock, { setupError: true }); await page.goto('/');
  await expect(page.getByRole('heading', { name: '暂时无法继续' })).toBeVisible();
  await expect(page.getByText(/无法读取安装信息：/)).toBeVisible();
  await layout(page);
});

test('uninstall confirmation traps focus, cancels on Escape, and keeps data by default', async ({ page }) => {
  await open(page, { mode: 'manage' });
  await page.locator('#manage-uninstall').click();
  await expect(page.getByRole('dialog')).toBeVisible();
  await expect(page.getByRole('checkbox', { name: /同时删除/ })).not.toBeChecked();
  for (let index = 0; index < 6; index++) {
    await page.keyboard.press('Tab');
    expect(await page.evaluate(() => !!document.activeElement?.closest('[role="dialog"]'))).toBe(true);
  }
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__calls.some((call: any) => call.command === 'start_uninstall'))).toBe(false);
});

test('update preserves settings and sends only the update operation', async ({ page }) => {
  await open(page, { mode: 'manage' }); await page.locator('#manage-update').click();
  await expect(page.getByRole('heading', { name: '准备更新' })).toBeVisible();
  await expect(page.locator('#install-steps')).toHaveCount(0);
  await expect(page.locator('#primary-button')).toBeEnabled(); await page.locator('#primary-button').click();
  await expect(page.getByRole('heading', { name: '更新已完成' })).toBeVisible();
  const call = await page.evaluate(() => (window as any).__calls.find((call: any) => call.command === 'start_install'));
  expect(call.args.update).toBe(true); expect(call.args.options.update).toBe(true);
  expect(call.args.options.selected_apps).toEqual([]); expect(call.args.options.enable_startup).toBe(false);
});

test('starting and running operations prevent duplicate requests and window closure', async ({ page }) => {
  await open(page, { holdOperation: true }); await confirm(page);
  await page.locator('#primary-button').dblclick();
  await expect(page.locator('#close-button')).toBeDisabled();
  expect(await page.evaluate(() => (window as any).__tryClose())).toBe(true);
  await expect(page.locator('#primary-button')).toBeDisabled();
  await expect(page.locator('#back-button')).toBeDisabled();
  await expect.poll(() => page.evaluate(() => (window as any).__calls.filter((call: any) => call.command === 'start_install').length)).toBe(1);
  await page.evaluate(() => { (window as any).__holdOperation = false; });
  await expect(page.getByRole('heading', { name: '安装已完成' })).toBeVisible();
  await expect(page.locator('#close-button')).toBeEnabled();
  expect(await page.evaluate(() => (window as any).__tryClose())).toBe(false);
  expect(await page.evaluate(() => (window as any).__calls.filter((call: any) => call.command === 'finish_operation').length)).toBe(1);
});

test('operation progress never moves backwards', async ({ page }) => {
  await open(page); await confirm(page); await page.locator('#primary-button').click();
  const values: number[] = [];
  for (let index = 0; index < 12; index++) {
    const value = await page.getByRole('progressbar').getAttribute('aria-valuenow');
    if (value !== null) values.push(Number(value));
    await page.waitForTimeout(100);
  }
  for (let index = 1; index < values.length; index++) expect(values[index]).toBeGreaterThanOrEqual(values[index - 1]);
  await expect(page.getByRole('heading', { name: '安装已完成' })).toBeVisible();
});

test('worker launch error restores controls and keeps long error details accessible', async ({ page }) => {
  await page.setViewportSize({ width: 560, height: 400 });
  await open(page, { startError: true }); await confirm(page); await page.locator('#primary-button').click();
  await expect(page.locator('#primary-button')).toHaveText('重试安装');
  await expect(page.locator('#close-button')).toBeEnabled();
  await expect(page.getByText(/无法启动工作进程：/)).toBeVisible();
  await layout(page);
});

test('failed operation can be retried without losing wizard selections', async ({ page }) => {
  await open(page, { failedOperation: true }); await confirm(page); await page.locator('#primary-button').click();
  await expect(page.locator('#primary-button')).toHaveText('重试安装');
  await expect(page.locator('#close-button')).toBeEnabled();
  expect(await page.evaluate(() => (window as any).__calls.some((call: any) => call.command === 'finish_operation'))).toBe(true);
});

test('confirmed uninstall preserves personal data unless explicitly selected', async ({ page }) => {
  await open(page, { mode: 'uninstall' });
  await expect(page.getByRole('dialog')).toBeVisible();
  await page.locator('#confirm-uninstall').click();
  await expect(page.getByRole('heading', { name: '卸载已完成' })).toBeVisible();
  const call = await page.evaluate(() => (window as any).__calls.find((call: any) => call.command === 'start_uninstall'));
  expect(call.args.deletePersonalData).toBe(false);
});
