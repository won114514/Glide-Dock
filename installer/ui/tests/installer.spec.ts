import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';
import path from 'node:path';
import { setupMock } from './mock';
import type { MockOptions } from './mock';

async function open(page: Page, options: MockOptions = {}) {
  await page.addInitScript(setupMock, options);
  await page.goto('/');
  await expect(page.locator('#version')).toHaveText('v0.4.4');
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

test('square window frame surrounds a single centered close button', async ({ page }) => {
  await open(page);
  await expect(page.locator('.setup-window')).toHaveCSS('border-radius', '0px');
  await expect(page.locator('.setup-window')).toHaveCSS('border-width', '1px');
  await expect(page.locator('.titlebar button')).toHaveCount(1);
  const close = page.getByRole('button', { name: '关闭', exact: true });
  const geometry = await close.evaluate(button => {
    const b = button.getBoundingClientRect();
    const i = button.querySelector('svg')!.getBoundingClientRect();
    return {
      size: [b.width, b.height], iconSize: [i.width, i.height],
      centerOffset: [i.x + i.width / 2 - (b.x + b.width / 2), i.y + i.height / 2 - (b.y + b.height / 2)],
    };
  });
  expect(geometry.size).toEqual([40, 32]);
  expect(geometry.iconSize).toEqual([28, 28]);
  expect(geometry.centerOffset).toEqual([0, 0]);
  await expect(close).toHaveCSS('border-radius', '0px');
  await close.click();
  expect(await page.evaluate(() => (window as any).__calls.some((call: any) => call.command === 'window.close'))).toBe(true);
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

test('folder picker cancellation preserves the path and allows browsing again', async ({ page }) => {
  await open(page, { browseResponses: [{ path: null, delay: 700 }, { path: 'D:\我的应用\Glide-Dock' }] });
  await expect(page.locator('#primary-button')).toBeEnabled();
  const originalPath = await page.locator('#install-path').inputValue();
  await page.locator('#browse-button').click();
  await expect(page.locator('#browse-button')).toBeDisabled();
  await expect(page.locator('#browse-button')).toHaveAttribute('aria-busy', 'true');
  await expect(page.locator('#primary-button')).toBeDisabled();
  await page.locator('#browse-button').dispatchEvent('click');
  expect(await page.evaluate(() => (window as any).__calls.filter((call: any) => call.command === 'browse_destination').length)).toBe(1);
  await expect(page.locator('#close-button')).toBeEnabled();
  await expect(page.locator('#browse-button')).toBeEnabled();
  await expect(page.locator('#install-path')).toHaveValue(originalPath);
  await expect(page.locator('#primary-button')).toBeEnabled();
  await page.locator('#browse-button').click();
  await expect(page.locator('#install-path')).toHaveValue('D:\我的应用\Glide-Dock');
});

test('folder picker errors preserve the path and permit a successful retry', async ({ page }) => {
  await open(page, { browseResponses: [{ error: '无法打开文件夹选择器：测试错误' }, { path: 'D:\重试\Glide-Dock' }] });
  const originalPath = await page.locator('#install-path').inputValue();
  await page.locator('#browse-button').click();
  await expect(page.getByText('无法打开文件夹选择器：测试错误', { exact: true })).toBeVisible();
  await expect(page.locator('#install-path')).toHaveValue(originalPath);
  await expect(page.locator('#browse-button')).toBeEnabled();
  await page.locator('#browse-button').click();
  await expect(page.locator('#install-path')).toHaveValue('D:\重试\Glide-Dock');
  await expect(page.locator('#primary-button')).toBeEnabled();
});

test('a late folder selection preserves newer manual path edits', async ({ page }) => {
  await open(page, { browseResponses: [{ path: 'D:\Old', delay: 500 }] });
  await page.locator('#browse-button').click();
  await page.locator('#install-path').fill('D:\Manual');
  await expect(page.locator('#browse-button')).toBeEnabled();
  await expect(page.locator('#install-path')).toHaveValue('D:\Manual');
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

test('long initialization error remains reachable without a visible scrollbar', async ({ page }) => {
  await page.setViewportSize({ width: 560, height: 400 });
  await page.addInitScript(setupMock, { setupError: true }); await page.goto('/');
  await expect(page.getByRole('heading', { name: '暂时无法继续' })).toBeVisible();
  await expect(page.getByText(/无法读取安装信息：/)).toBeVisible();
  const content = page.locator('#main-content');
  await expect(content).toHaveCSS('scrollbar-width', 'none');
  expect(await content.evaluate(el => el.offsetWidth - el.clientWidth)).toBe(0);
  await content.hover({ position: { x: 16, y: 16 } });
  await page.mouse.wheel(0, 120);
  await expect.poll(() => content.evaluate(el => el.scrollTop)).toBeGreaterThan(0);
  await content.focus();
  await page.keyboard.press('Home');
  await expect(content).toHaveJSProperty('scrollTop', 0);
  await page.keyboard.press('PageDown');
  await expect.poll(() => content.evaluate(el => el.scrollTop)).toBeGreaterThan(0);
  await page.keyboard.press('End');
  await expect(page.getByRole('button', { name: '重新检查' })).toBeInViewport();
  await layout(page);
});

test('uninstall confirmation traps focus, cancels on Escape, and keeps data by default', async ({ page }) => {
  await open(page, { mode: 'uninstall' });
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
  await open(page, { mode: 'update' });
  await expect(page.getByRole('heading', { name: '准备更新' })).toBeVisible();
  await expect(page.locator('#install-steps')).toHaveCount(0);
  await expect(page.locator('#primary-button')).toBeEnabled(); await page.locator('#primary-button').click();
  await expect(page.getByRole('heading', { name: '更新已完成' })).toBeVisible();
  const call = await page.evaluate(() => (window as any).__calls.find((call: any) => call.command === 'start_install'));
  expect(call.args.update).toBe(true); expect(call.args.options.update).toBe(true);
  expect(call.args.options.selected_apps).toEqual([]); expect(call.args.options.enable_startup).toBe(false);
});

test('auto-update argument skips checklist, runs preflight, and enters update progress', async ({ page }) => {
  await open(page, { mode: 'update', autoUpdate: true, holdOperation: true });
  await expect(page.locator('#auto-update-page')).toBeVisible();
  await expect(page.locator('#confirm-page')).toHaveCount(0);
  await expect(page.locator('#setup-page')).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => (window as any).__calls.some((call: any) => call.command === 'check_destination' && call.args.update === true))).toBe(true);
  await expect(page.getByRole('progressbar', { name: '操作进度' })).toBeVisible();
  await expect(page.locator('#operation-message')).toHaveAttribute('title', '正在处理程序文件…');
  const call = await page.evaluate(() => (window as any).__calls.find((call: any) => call.command === 'start_install'));
  expect(call.args.update).toBe(true);
  expect(call.args.options.update).toBe(true);
  expect(call.args.options.launch_after_install).toBe(true);
});

test('auto-update stops before starting when preflight checks fail', async ({ page }) => {
  await open(page, { mode: 'update', autoUpdate: true, failedCheck: true });
  await expect(page.locator('#auto-update-page')).toBeVisible();
  await expect(page.getByText('安装包数据不完整，请重新下载。')).toBeVisible();
  await page.waitForTimeout(250);
  expect(await page.evaluate(() => (window as any).__calls.some((call: any) => call.command === 'start_install'))).toBe(false);
});

test('auto-update reports blocked installed-version eligibility instead of silently waiting', async ({ page }) => {
  await open(page, { mode: 'update', autoUpdate: true, updateBlocked: 'same' });
  await expect(page.locator('#auto-update-page')).toBeVisible();
  await expect(page.getByText(/请使用更高版本的安装包/).first()).toBeVisible();
  await page.waitForTimeout(250);
  expect(await page.evaluate(() => (window as any).__calls.some((call: any) => call.command === 'start_install'))).toBe(false);
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

for (const colorScheme of ['light', 'dark'] as const) {
  test(colorScheme + ' close hover changes only the titlebar', async ({ page }) => {
    await page.emulateMedia({ colorScheme });
    await open(page, { mode: 'update' });
    const close = page.getByRole('button', { name: '关闭', exact: true });
    const icon = close.locator('svg');
    const normal = colorScheme === 'dark' ? 'rgb(170, 166, 182)' : 'rgb(141, 139, 155)';
    const hovered = colorScheme === 'dark' ? 'rgb(238, 235, 245)' : 'rgb(95, 89, 110)';
    const summary = page.locator('.summary-card');
    await expect(summary).toBeVisible();
    await expect(icon).toHaveCSS('color', normal);
    const before = await summary.screenshot({ animations: 'disabled' });
    await close.hover();
    await expect(icon).toHaveCSS('color', hovered);
    await expect(page.getByRole('tooltip')).toHaveCount(0);
    const during = await summary.screenshot({ animations: 'disabled' });
    expect(during.equals(before), 'close hover must not cover or repaint the page').toBe(true);
    await page.mouse.move(20, 100);
    await expect(icon).toHaveCSS('color', normal);
  });
}

test('rounded cards keep their outlines aligned with the surface', async ({ page }) => {
  await open(page);
  await preferences(page);
  for (const selector of ['.preference-settings', '.summary-card']) {
    const card = page.locator(selector);
    await expect(card).toBeVisible();
    const radii = await card.evaluate(element => ({ surface: getComputedStyle(element).borderRadius, outline: getComputedStyle(element, '::after').borderRadius }));
    expect(parseFloat(radii.surface)).toBeGreaterThan(0);
    expect(radii.outline).toBe(radii.surface);
    if (selector === '.preference-settings') await page.locator('#primary-button').click();
  }
});

test('right click suppresses browser menus and keeps path editing available', async ({ page }) => {
  await open(page);
  await page.evaluate(() => {
    (window as any).__contextMenus = [];
    document.addEventListener('contextmenu', event => (window as any).__contextMenus.push(event.defaultPrevented));
  });
  await page.locator('#main-content').click({ button: 'right', position: { x: 12, y: 12 } });
  await page.locator('#install-path').click({ button: 'right' });
  await page.locator('#close-button').click({ button: 'right' });
  expect(await page.evaluate(() => (window as any).__contextMenus)).toEqual([true, true, true]);
  await page.locator('#install-path').focus();
  await page.keyboard.press('Control+A');
  await page.keyboard.insertText('D:\\Apps\\Glide-Dock');
  await expect(page.locator('#install-path')).toHaveValue('D:\\Apps\\Glide-Dock');
});

test('refresh shortcuts preserve installation inputs and the current step', async ({ page }) => {
  await open(page);
  await page.evaluate(() => {
    const prevented: boolean[] = [];
    (window as any).__refreshPrevented = prevented;
    document.addEventListener('keydown', event => {
      if (event.key === 'F5' || event.key.toLowerCase() === 'r') prevented.push(event.defaultPrevented);
    });
  });
  const path = page.locator('#install-path');
  await path.fill('D:\\Apps\\Glide-Dock');
  for (const shortcut of ['F5', 'Control+F5', 'Control+r', 'Control+Shift+R']) {
    await page.keyboard.press(shortcut);
    await expect(path).toHaveValue('D:\\Apps\\Glide-Dock');
  }
  await preferences(page);
  await page.keyboard.press('F5');
  await expect(page.locator('#setup-page')).toBeVisible();
  expect(await page.evaluate(() => (window as any).__refreshPrevented)).toEqual([true, true, true, true, true]);
});

for (const [blocked, title] of [['same', '已安装此版本'], ['older', '安装包版本较旧'], ['unknown', '无法判断已安装版本']] as const) {
  test('blocks update for ' + blocked + ' installed version', async ({ page }) => {
    await open(page, { mode: 'update', updateBlocked: blocked });
    await expect(page.getByRole('heading', { name: title })).toBeVisible();
    await expect(page.locator('#primary-button')).toBeDisabled();
    await expect(page.getByText('安装包版本', { exact: true })).toBeVisible();
    await page.waitForTimeout(300);
    expect(await page.evaluate(() => (window as any).__calls.some((call: any) => call.command === 'start_install'))).toBe(false);
    await expect(page.getByText(/卸载请使用/).first()).toBeVisible();
  });
}

test('standalone uninstaller exposes no update or installation controls', async ({ page }) => {
  await open(page, { mode: 'uninstall', missingPayload: true });
  await page.keyboard.press('Escape');
  await expect(page).toHaveTitle('Glide-Dock 卸载程序');
  await expect(page.locator('#uninstall-view')).toBeVisible();
  await expect(page.locator('#manage-update')).toHaveCount(0);
  await expect(page.getByText('安装环境', { exact: true })).toHaveCount(0);
  await expect(page.getByText(/此构建未包含安装文件/)).toHaveCount(0);
  await expect(page.locator('#install-steps')).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__calls.some((call: any) => ['start_install', 'check_destination', 'browse_destination'].includes(call.command)))).toBe(false);
});


test('uninstall cleanup warnings finish successfully and do not offer a misleading retry', async ({ page }) => {
  await open(page, { mode: 'uninstall', cleanupWarning: true });
  await page.locator('#confirm-uninstall').click();
  await expect(page.getByRole('heading', { name: '卸载已完成' })).toBeVisible();
  await expect(page.locator('#primary-button')).toHaveText('关闭');
  await expect(page.locator('.feedback-details')).toContainText('空目录暂时无法删除');
  await expect(page.getByRole('button', { name: '重试卸载' })).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__calls.filter((call: any) => call.command === 'start_uninstall').length)).toBe(1);
});

test('a partial uninstall can be retried in the same window', async ({ page }) => {
  await open(page, { mode: 'uninstall', failedOperation: true });
  await page.locator('#confirm-uninstall').click();
  await expect(page.locator('#primary-button')).toHaveText('重试卸载');
  await page.evaluate(() => { (window as any).__failedOperation = false; });
  await page.locator('#primary-button').click();
  await page.locator('#confirm-uninstall').click();
  await expect(page.getByRole('heading', { name: '卸载已完成' })).toBeVisible();
  expect(await page.evaluate(() => (window as any).__calls.filter((call: any) => call.command === 'start_uninstall').length)).toBe(2);
});
