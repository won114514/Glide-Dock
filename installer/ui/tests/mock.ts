export interface MockOptions {
  cleanupWarning?: boolean;
  mode?: 'install' | 'update' | 'uninstall';
  autoUpdate?: boolean;
  updateBlocked?: 'same' | 'older' | 'unknown';
  setupError?: boolean;
  missingPayload?: boolean;
  failedCheck?: boolean;
  emptyApps?: boolean;
  startError?: boolean;
  holdOperation?: boolean;
  failedOperation?: boolean;
  scopeDelay?: number;
  browseResponses?: { path?: string | null; error?: string; delay?: number }[];
}

export function setupMock(options: MockOptions = {}) {
  const win = window as any;
  const mode = options.mode ?? 'install';
  const existing = mode === 'install' ? null : { scope: 'current-user', version: options.updateBlocked === 'same' ? '0.4.4' : options.updateBlocked === 'older' ? '0.5.0' : options.updateBlocked === 'unknown' ? null : '0.4.3', install_directory: 'C:\\Users\\演示用户\\AppData\\Local\\Programs\\Glide-Dock' };
  const info = {
    version: '0.4.4', mode, auto_update: !!options.autoUpdate, payload_ready: mode !== 'uninstall' && !options.missingPayload,
    update_eligibility: mode === 'update' ? {
      allowed: !options.updateBlocked,
      title: options.updateBlocked === 'same' ? '已安装此版本' : options.updateBlocked === 'older' ? '安装包版本较旧' : options.updateBlocked === 'unknown' ? '无法判断已安装版本' : '准备更新',
      message: options.updateBlocked ? '请使用更高版本的安装包。卸载请使用 Windows“已安装的应用”。' : '将 Glide-Dock 从 0.4.3 更新到 0.4.4，保留当前设置和 Dock 条目。',
    } : null,
    install_directory: existing?.install_directory ?? 'C:\\Users\\演示用户\\AppData\\Local\\Programs\\Glide-Dock', existing,
    checks: [{ name: '系统兼容性', passed: true, message: 'Windows 11，64 位系统。' },
      { name: '安装文件', passed: !options.failedCheck, message: options.failedCheck ? '安装包数据不完整，请重新下载。' : '安装文件可用，磁盘空间充足。' }],
    apps: options.emptyApps ? [] : Array.from({ length: 18 }, (_, index) => ({ name: index ? '示例应用 ' + index : '用于检查布局的很长的应用程序名称以及附加说明', path: 'C:\\Apps\\app' + index + '.exe', source: '开始菜单' })),
  };
  const calls: any[] = [];
  win.__calls = calls;
  win.__holdOperation = !!options.holdOperation;
  win.__failedOperation = !!options.failedOperation;
  let closeRequested: ((event: any) => void) | undefined;
  let progressRead = 0;
  let browseRead = 0;
  win.__tryClose = () => {
    let prevented = false;
    closeRequested?.({ preventDefault: () => { prevented = true; } });
    return prevented;
  };
  const sleep = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));
  win.__TAURI__ = {
    core: { invoke: async (command: string, args: any = {}) => {
      calls.push({ command, args });
      if (command === 'setup_info') {
        if (options.setupError) throw new Error('无法读取安装信息：' + 'C:\\很长的错误路径\\'.repeat(24));
        return info;
      }
      if (command === 'default_destination') {
        await sleep(options.scopeDelay ?? 5);
        return args.scope === 'all-users' ? 'C:\\Program Files\\Glide-Dock' : info.install_directory;
      }
      if (command === 'browse_destination') {
        const response = options.browseResponses?.[browseRead++];
        await sleep(response?.delay ?? 0);
        if (response?.error) throw new Error(response.error);
        return response?.path === undefined ? 'D:\\我的应用\\Glide-Dock' : response.path;
      }
      if (command === 'check_destination') {
        await sleep(args.installDirectory.includes('slow') ? 350 : 10);
        const valid = !args.installDirectory.includes('invalid');
        return { valid, message: valid ? '此位置可以安装。' : '此文件夹不可写，请选择其他位置。', space: valid ? '可用 128 GB' : '' };
      }
      if (command === 'start_install' || command === 'start_uninstall') {
        await sleep(120);
        if (options.startError) throw new Error('无法启动工作进程：' + '拒绝访问 C:\\很长的目录\\'.repeat(24));
        return '123-456';
      }
      if (command === 'operation_status') {
        await sleep(10);
        if (win.__holdOperation) return { percent: 35, phase: 'running', message: '正在处理程序文件…' };
        if (win.__failedOperation) return { percent: 35, phase: 'failed', message: '文件被占用，请关闭应用后重试。' };
        if (options.cleanupWarning) return { percent: 100, phase: 'complete', message: 'Glide-Dock 已卸载。程序文件已移除，但空目录暂时无法删除。', warnings: ['空目录清理未完成'] };
        const percent = [55, 20, 100][Math.min(progressRead++, 2)];
        return { percent, phase: percent === 100 ? 'complete' : 'running', message: percent === 100 ? '操作已完成。' : '正在处理程序文件…' };
      }
      if (command === 'finish_operation') return null;
      throw new Error('Unexpected backend command: ' + command);
    } },
    window: { getCurrentWindow: () => ({
      close: async () => { calls.push({ command: 'window.close' }); },
      onCloseRequested: async (handler: (event: any) => void) => { closeRequested = handler; return () => { closeRequested = undefined; }; },
    }) },
  };
}
