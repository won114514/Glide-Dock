export interface MockOptions {
  mode?: 'install' | 'manage' | 'update' | 'uninstall';
  setupError?: boolean;
  missingPayload?: boolean;
  failedCheck?: boolean;
  emptyApps?: boolean;
  startError?: boolean;
  holdOperation?: boolean;
  failedOperation?: boolean;
  scopeDelay?: number;
}

export function setupMock(options: MockOptions = {}) {
  const win = window as any;
  const mode = options.mode ?? 'install';
  const existing = mode === 'install' ? null : { scope: 'current-user', install_directory: 'C:\\Users\\演示用户\\AppData\\Local\\Programs\\Glide-Dock' };
  const info = {
    version: '0.4.2', mode, payload_ready: !options.missingPayload,
    install_directory: existing?.install_directory ?? 'C:\\Users\\演示用户\\AppData\\Local\\Programs\\Glide-Dock', existing,
    checks: [{ name: '系统兼容性', passed: true, message: 'Windows 11，64 位系统。' },
      { name: '安装文件', passed: !options.failedCheck, message: options.failedCheck ? '安装包数据不完整，请重新下载。' : '安装文件可用，磁盘空间充足。' }],
    apps: options.emptyApps ? [] : Array.from({ length: 18 }, (_, index) => ({ name: index ? '示例应用 ' + index : '用于检查布局的很长的应用程序名称以及附加说明', path: 'C:\\Apps\\app' + index + '.exe', source: '开始菜单' })),
  };
  const calls: any[] = [];
  win.__calls = calls;
  win.__holdOperation = !!options.holdOperation;
  let maximized = false;
  let resize: (() => void) | undefined;
  let closeRequested: ((event: any) => void) | undefined;
  let progressRead = 0;
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
      if (command === 'browse_destination') return 'D:\\我的应用\\Glide-Dock';
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
        if (options.failedOperation) return { percent: 35, phase: 'failed', message: '文件被占用，请关闭应用后重试。' };
        const percent = [55, 20, 100][Math.min(progressRead++, 2)];
        return { percent, phase: percent === 100 ? 'complete' : 'running', message: percent === 100 ? '操作已完成。' : '正在处理程序文件…' };
      }
      if (command === 'finish_operation') return null;
      throw new Error('Unexpected backend command: ' + command);
    } },
    window: { getCurrentWindow: () => ({
      isMaximized: async () => maximized,
      toggleMaximize: async () => { maximized = !maximized; resize?.(); },
      minimize: async () => { calls.push({ command: 'window.minimize' }); },
      close: async () => { calls.push({ command: 'window.close' }); },
      onResized: async (handler: () => void) => { resize = handler; return () => { resize = undefined; }; },
      onCloseRequested: async (handler: (event: any) => void) => { closeRequested = handler; return () => { closeRequested = undefined; }; },
    }) },
  };
}
