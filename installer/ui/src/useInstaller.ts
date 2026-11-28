import { useEffect, useRef, useState } from 'react';
import { invoke, setupWindow } from './backend';
import type { DestinationCheck, Mode, OperationStatus, Page, Scope, SetupInfo, StartupMode, Theme } from './backend';

export type FeedbackIntent = 'info' | 'success' | 'warning' | 'error';
const pageOrder: Page[] = ['location', 'setup', 'confirm'];
const errorText = (error: unknown) => error instanceof Error ? error.message : String(error);

export function useInstaller() {
  const [info, setInfo] = useState<SetupInfo | null>(null);
  const [loadError, setLoadError] = useState('');
  const [mode, setMode] = useState<Mode>('install');
  const [page, setPage] = useState<Page>('location');
  const [furthestPage, setFurthestPage] = useState(0);
  const [scope, setScope] = useState<Scope>('current-user');
  const [installPath, setInstallPath] = useState('');
  const [destination, setDestination] = useState<DestinationCheck>({ valid: false, message: '正在检查安装位置…', space: '' });
  const [checking, setChecking] = useState(true);
  const [theme, setTheme] = useState<Theme>('system');
  const [startupEnabled, setStartupEnabled] = useState(true);
  const [startupMode, setStartupMode] = useState<StartupMode>('task');
  const [desktopShortcut, setDesktopShortcut] = useState(false);
  const [launchAfterInstall, setLaunchAfterInstall] = useState(true);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [query, setQuery] = useState('');
  const [appsOpen, setAppsOpen] = useState(false);
  const [uninstallOpen, setUninstallOpen] = useState(false);
  const [deletePersonalData, setDeletePersonalData] = useState(false);
  const [feedback, setFeedback] = useState<{ message: string; intent: FeedbackIntent }>({ message: '正在读取安装信息…', intent: 'info' });
  const [pending, setPending] = useState(false);
  const [browsing, setBrowsing] = useState(false);
  const browsingRef = useRef(false);
  const [token, setToken] = useState<string | null>(null);
  const [progress, setProgress] = useState<number | null>(null);
  const [phase, setPhase] = useState<'idle' | 'running' | 'complete' | 'failed'>('idle');
  const busyRef = useRef(false);
  const autoStarted = useRef(false);
  const scopeRequest = useRef(0);
  const lastProgress = useRef(0);
  const busy = pending || token !== null;
  const complete = phase === 'complete';
  const canInstall = !!info?.payload_ready && info.checks.every(check => check.passed) && (mode !== 'update' || !!info.update_eligibility?.allowed);

  useEffect(() => {
    let disposed = false;
    invoke<SetupInfo>('setup_info').then(result => {
      if (disposed) return;
      setInfo(result);
      setMode(result.mode);
      document.title = result.mode === 'uninstall' ? 'Glide-Dock 卸载程序' : 'Glide-Dock 安装程序';
      if (result.mode === 'uninstall') setChecking(false);
      setScope(result.existing?.scope ?? 'current-user');
      setInstallPath(result.install_directory);
      setPage(result.mode === 'update' ? 'confirm' : 'location');
      setUninstallOpen(result.mode === 'uninstall');
      setFeedback({
        message: result.mode === 'uninstall' ? '默认保留当前账户的设置、Dock 条目和日志。'
          : result.mode === 'update' ? result.update_eligibility?.message ?? '无法读取版本信息。' : '请选择安装位置。',
        intent: result.mode === 'uninstall' || (result.mode === 'update' && !result.update_eligibility?.allowed) ? 'warning' : 'info',
      });
    }).catch(error => {
      if (disposed) return;
      setLoadError('无法读取安装信息：' + errorText(error));
      setFeedback({ message: '暂时无法继续，请重新检查安装信息。', intent: 'error' });
    });
    return () => { disposed = true; };
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    setupWindow.onCloseRequested(event => {
      if (busyRef.current) event.preventDefault();
    }).then(dispose => { if (disposed) dispose(); else unlisten = dispose; }).catch(console.error);
    return () => { disposed = true; unlisten?.(); };
  }, []);

  useEffect(() => {
    if (!info || !['install', 'update'].includes(mode) || busy || complete) return;
    if (mode === 'update' && !info.update_eligibility?.allowed) { setChecking(false); return; }
    let cancelled = false;
    setChecking(true);
    setDestination({ valid: false, message: '正在检查安装位置…', space: '' });
    const timer = window.setTimeout(async () => {
      try {
        const result = await invoke<DestinationCheck>('check_destination', { installDirectory: installPath.trim(), scope, update: mode === 'update' });
        if (!cancelled) setDestination(result);
      } catch (error) {
        if (!cancelled) setDestination({ valid: false, message: errorText(error), space: '' });
      } finally {
        if (!cancelled) setChecking(false);
      }
    }, 180);
    return () => { cancelled = true; window.clearTimeout(timer); };
  }, [info, installPath, scope, mode, busy, complete]);

  useEffect(() => {
    if (!info?.auto_update || mode !== 'update' || phase !== 'idle' || busyRef.current
      || checking || !destination.valid || !canInstall || autoStarted.current) return;
    autoStarted.current = true;
    void beginInstall();
  }, [info, mode, phase, checking, destination.valid, canInstall]);

  useEffect(() => {
    if (!token) return;
    let cancelled = false;
    let timer: number | undefined;
    const poll = async () => {
      try {
        const status = await invoke<OperationStatus>('operation_status', { token });
        if (cancelled) return;
        const percent = status.phase === 'complete' ? 100 : Math.min(99, Math.max(lastProgress.current, status.percent));
        lastProgress.current = percent;
        setProgress(percent);
        setFeedback({ message: status.message, intent: status.phase === 'failed' ? 'error' : status.warnings?.length ? 'warning' : status.phase === 'complete' ? 'success' : 'info' });
        if (status.phase === 'running') {
          timer = window.setTimeout(poll, 350);
          return;
        }
        await invoke('finish_operation', { token });
        if (cancelled) return;
        busyRef.current = false;
        setToken(null);
        setPhase(status.phase);
      } catch (error) {
        if (cancelled) return;
        setFeedback({ message: '正在重试读取操作状态：' + errorText(error), intent: 'warning' });
        timer = window.setTimeout(poll, 800);
      }
    };
    void poll();
    return () => { cancelled = true; window.clearTimeout(timer); };
  }, [token]);

  async function changeScope(next: Scope) {
    if (busyRef.current || mode !== 'install') return;
    const request = ++scopeRequest.current;
    setScope(next);
    try {
      const path = await invoke<string>('default_destination', { scope: next });
      if (request === scopeRequest.current) setInstallPath(path);
    } catch (error) {
      if (request === scopeRequest.current) setFeedback({ message: errorText(error), intent: 'error' });
    }
  }

  function changePath(path: string) {
    scopeRequest.current += 1;
    setInstallPath(path);
  }

  async function browse() {
    if (busyRef.current || browsingRef.current) return;
    browsingRef.current = true;
    setBrowsing(true);
    const request = ++scopeRequest.current;
    try {
      const path = await invoke<string | null>('browse_destination');
      if (path && request === scopeRequest.current) setInstallPath(path);
    } catch (error) { setFeedback({ message: errorText(error), intent: 'error' }); }
    finally {
      browsingRef.current = false;
      setBrowsing(false);
    }
  }

  function enterPage(next: Page) {
    setPage(next);
    if (phase === 'idle') setFeedback({ message: {
      location: '请选择安装位置。', setup: '这些设置也可以在安装后调整。', confirm: '确认安装清单后即可开始。',
    }[next], intent: 'info' });
  }

  function navigate(next: Page) {
    if (!busyRef.current && pageOrder.indexOf(next) <= furthestPage) enterPage(next);
  }

  function nextPage() {
    if (busyRef.current) return;
    const index = pageOrder.indexOf(page);
    if (index === 0 && (!destination.valid || checking)) return;
    if (index < 2) {
      setFurthestPage(Math.max(furthestPage, index + 1));
      enterPage(pageOrder[index + 1]);
    }
  }

  function toggleApp(index: number, checked: boolean) {
    setSelected(previous => {
      const next = new Set(previous);
      if (checked) next.add(index); else next.delete(index);
      return next;
    });
  }
  const normalizedQuery = query.trim().toLocaleLowerCase();
  const visibleApps = (info?.apps ?? []).map((app, index) => ({ app, index })).filter(({ app }) => (app.name + ' ' + app.source).toLocaleLowerCase().includes(normalizedQuery));
  function selectVisible() { setSelected(previous => new Set([...previous, ...visibleApps.map(({ index }) => index)])); }

  async function startOperation(command: string, args: Record<string, unknown>) {
    if (busyRef.current || !info) return;
    busyRef.current = true;
    setPending(true);
    setPhase('running');
    setProgress(1);
    lastProgress.current = 1;
    setFeedback({ message: scope === 'all-users' ? '等待 Windows 管理员确认…' : '正在准备操作…', intent: 'info' });
    try {
      const operation = await invoke<string>(command, args);
      setToken(operation);
    } catch (error) {
      busyRef.current = false;
      setPhase('failed');
      setProgress(null);
      setFeedback({ message: errorText(error), intent: 'error' });
    } finally { setPending(false); }
  }

  async function beginInstall() {
    if (!destination.valid || checking || !canInstall || !info) return;
    const updating = mode === 'update';
    await startOperation('start_install', { options: {
      scope, install_directory: installPath.trim(), create_desktop_shortcut: !updating && desktopShortcut,
      enable_startup: !updating && startupEnabled, startup_mode: startupMode,
      launch_after_install: updating || launchAfterInstall, theme,
      selected_apps: updating ? [] : info.apps.filter((_, index) => selected.has(index)), update: updating,
    }, update: updating });
  }

  async function beginUninstall() {
    if (busyRef.current) return;
    setUninstallOpen(false);
    setMode('uninstall');
    await startOperation('start_uninstall', { deletePersonalData });
  }

  async function close() { if (!busyRef.current) await setupWindow.close(); }
  let primaryLabel = '正在读取…';
  let primaryDisabled = !info || busy;
  let primaryAction = nextPage;
  if (loadError) primaryLabel = '暂时无法继续';
  else if (complete) { primaryLabel = '关闭'; primaryDisabled = false; primaryAction = () => { void close(); }; }
  else if (busy) primaryLabel = mode === 'uninstall' ? '正在卸载…' : mode === 'update' ? '正在更新…' : '正在安装…';
  else if (info) {
    if (mode === 'uninstall') { primaryLabel = phase === 'failed' ? '重试卸载' : '确认卸载…'; primaryAction = () => setUninstallOpen(true); }
    else if (mode === 'update' || page === 'confirm') {
      primaryLabel = mode === 'update' ? (phase === 'failed' ? '重试更新' : info.auto_update ? '正在准备更新…' : '安装更新') : (phase === 'failed' ? '重试安装' : '开始安装');
      primaryDisabled = info.auto_update && phase !== 'failed' || !destination.valid || checking || !canInstall;
      primaryAction = () => { void beginInstall(); };
    } else { primaryLabel = page === 'location' ? '下一步' : '查看安装清单'; primaryDisabled = page === 'location' && (!destination.valid || checking); }
  }

  return {
    info, loadError, mode, page, furthestPage, scope, installPath, destination, checking, theme, startupEnabled,
    startupMode, desktopShortcut, launchAfterInstall, selected, query, uninstallOpen, deletePersonalData,
    feedback, busy, browsing, complete, canInstall, progress, phase, visibleApps, appsOpen, setAppsOpen, primaryLabel, primaryDisabled: primaryDisabled || browsing, primaryAction,
    changeScope, changePath, browse, navigate, nextPage, toggleApp, selectVisible, close, beginUninstall,
    setTheme, setStartupEnabled, setStartupMode, setDesktopShortcut, setLaunchAfterInstall, setQuery,
    setSelected, setUninstallOpen, setDeletePersonalData,
  };
}
export type InstallerState = ReturnType<typeof useInstaller>;
