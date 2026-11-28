import { useEffect, useRef, useState } from 'react';
import {
  Accordion, AccordionHeader, AccordionItem, AccordionPanel, Badge, Button, Caption1, Card, Checkbox, Dialog, DialogActions, DialogBody, DialogContent,
  DialogSurface, DialogTitle, Divider, Field, FluentProvider, Input, MessageBar, MessageBarBody,
  ProgressBar, Radio, RadioGroup, Select, Spinner, Subtitle2, Switch, Tab, TabList, Text,
  webDarkTheme, webLightTheme,
} from '@fluentui/react-components';
import {
  Apps20Regular, ArrowLeft16Regular, ArrowRight16Regular, Checkmark16Regular, Color20Regular,
  CheckmarkCircle16Regular, CheckmarkCircle32Regular, ChevronRight16Regular, Delete20Regular,
  Dismiss24Regular, ErrorCircle16Regular, FolderOpen20Regular, Info16Regular, Play20Regular, Power20Regular, ShieldCheckmark20Regular,
  Search16Regular, Warning16Regular,
} from '@fluentui/react-icons';
import brandMark from '../../../assets/brand/01-Logo/Glide-Dock-Logo-Symbol.svg';
import type { Page, Scope, StartupMode, Theme } from './backend';
import { useInstaller } from './useInstaller';
import type { FeedbackIntent, InstallerState } from './useInstaller';
import '../setup.css';

function StatusIcon({ intent }: { intent: FeedbackIntent }) {
  return <span className={'status-icon status-' + intent} aria-hidden="true">
    {intent === 'success' ? <CheckmarkCircle16Regular /> : intent === 'error' ? <ErrorCircle16Regular />
      : intent === 'warning' ? <Warning16Regular /> : <Info16Regular />}
  </span>;
}

function Titlebar({ state }: { state: InstallerState }) {
  const title = state.complete ? '操作已完成' : state.mode === 'update' ? '更新' : state.mode === 'uninstall' ? '卸载' : '安装';
  return <header className="titlebar">
    <div className="brand-lockup" data-tauri-drag-region>
      <img className="brand-mark" src={brandMark} alt="" />
      <Text weight="semibold">Glide-Dock</Text>
      <Caption1 className="titlebar-subtitle" id="page-title">{title}</Caption1>
      {state.info && <Caption1 className="version" id="version">v{state.info.version}</Caption1>}
    </div>
    <div className="titlebar-drag" data-tauri-drag-region />
    <div className="window-controls" aria-label="窗口控制">
      <Button id="close-button" className="close-button" appearance="subtle" size="small"
        aria-label="关闭" disabled={state.busy} onClick={() => { void state.close(); }}>
        <Dismiss24Regular aria-hidden="true" />
      </Button>
    </div>
  </header>;
}

const steps: { page: Page; label: string; description: string }[] = [
  { page: 'location', label: '安装位置', description: '选择范围与文件夹' },
  { page: 'setup', label: '个性化设置', description: '按你的习惯开始' },
  { page: 'confirm', label: '准备安装', description: '确认后即可开始' },
];
function Sidebar({ state }: { state: InstallerState }) {
  const active = steps.findIndex(step => step.page === state.page);
  const showSteps = state.mode === 'install' && !state.complete && !!state.info;
  return <aside className="sidebar">
    <div className="sidebar-heading"><Caption1>GLIDE-DOCK</Caption1><Text weight="semibold">{showSteps ? '开始使用' : state.complete ? '一切就绪' : state.mode === 'uninstall' ? '卸载应用' : '更新应用'}</Text></div>
    {showSteps ? <nav className="steps" id="install-steps" aria-label="安装步骤">
      <TabList vertical selectedValue={state.page} onTabSelect={(_, data) => state.navigate(data.value as Page)} size="large" appearance="subtle">
        {steps.map((step, index) => <Tab className="step-tab" id={'step-' + step.page} key={step.page} value={step.page}
          disabled={state.busy || index > state.furthestPage} aria-current={index === active ? 'step' : undefined}
          icon={<Badge className="step-badge" appearance={index === active ? 'filled' : index < active ? 'tint' : 'outline'} color={index < active ? 'success' : index === active ? 'brand' : 'subtle'}>
            {index < active ? <Checkmark16Regular data-completed-step="true" /> : index + 1}
          </Badge>}>
          <span className="step-copy"><span>{step.label}</span><Caption1>{step.description}</Caption1></span>
        </Tab>)}
      </TabList>
    </nav> : <div className="sidebar-current"><span className="sidebar-current-icon">{state.complete ? <CheckmarkCircle32Regular /> : <Apps20Regular />}</span><Text>{state.complete ? '操作已完成' : state.mode === 'uninstall' ? '卸载 Glide-Dock' : '更新 Glide-Dock'}</Text><Caption1>{state.complete ? '现在可以关闭此窗口' : state.mode === 'uninstall' ? '移除程序，默认保留设置' : '使用新版安装包更新'}</Caption1></div>}
    <div className="sidebar-note"><ShieldCheckmark20Regular /><span><Text size={200}>为 Windows 设计</Text><Caption1>Glide-Dock{state.info ? ` · v${state.info.version}` : ''}</Caption1></span></div>
  </aside>;
}

function Intro({ title, children }: { title: string; children: React.ReactNode }) {
  return <div className="page-intro"><Text as="h2" size={600} weight="semibold" tabIndex={-1}>{title}</Text><Text className="muted" size={300}>{children}</Text></div>;
}

function Location({ state: s }: { state: InstallerState }) {
  return <section className="page location-page" id="location-page">
    <Intro title="安装 Glide-Dock">为桌面添一个轻巧的 Dock，从这里开始。</Intro>
    <div className="location-form">
      <Field label="为谁安装">
        <RadioGroup name="scope" value={s.scope} className="scope-picker" disabled={s.busy}
          onChange={(_, data) => { void s.changeScope(data.value as Scope); }}>
          <Radio value="current-user" label={<span className="radio-copy"><Text weight="semibold">仅为我安装</Text><Caption1 className="muted">只对当前 Windows 账户可用</Caption1></span>} />
          <Radio value="all-users" label={<span className="radio-copy"><Text weight="semibold">为所有用户安装</Text><Caption1 className="muted">开始安装时请求管理员确认</Caption1></span>} />
        </RadioGroup>
      </Field>
      <Field label="安装文件夹" validationState={s.checking ? 'none' : s.destination.valid ? 'success' : 'error'}
        validationMessage={s.destination.message + (s.destination.space ? ' · ' + s.destination.space : '')}>
        <div className="path-row">
          <Input id="install-path" value={s.installPath} onChange={(_, data) => s.changePath(data.value)} spellCheck={false} autoComplete="off" disabled={s.busy} aria-label="安装文件夹" />
          <Button id="browse-button" icon={<FolderOpen20Regular />} disabled={s.busy || s.browsing} aria-busy={s.browsing} onClick={() => { void s.browse(); }}>浏览…</Button>
        </div>
      </Field>
      <Checkbox id="desktop-shortcut" label="在桌面创建快捷方式" checked={s.desktopShortcut} disabled={s.busy} onChange={(_, data) => s.setDesktopShortcut(!!data.checked)} />
    </div>
  </section>;
}

function Preferences({ state: s }: { state: InstallerState }) {
  return <section className="page" id="setup-page">
    <Intro title="让它更合你的习惯">先选好外观与启动方式，以后也能随时调整。</Intro>
    <Card appearance="outline" className="preference-settings">
      <div className="preference-row">
        <span className="preference-icon"><Color20Regular /></span><div className="preference-copy"><Text weight="medium">颜色主题</Text><Caption1>选择 Dock 的外观</Caption1></div>
        <Select className="preference-select" aria-label="Dock 颜色主题" value={s.theme} onChange={(_, data) => s.setTheme(data.value as Theme)} disabled={s.busy}>
          <option value="system">跟随系统</option><option value="light">浅色</option><option value="dark">深色</option>
        </Select>
      </div>
      <div className="preference-row startup-row">
        <span className="preference-icon"><Power20Regular /></span><div className="preference-copy"><Text weight="medium">登录时自动启动</Text><Caption1>进入桌面后，Dock 随时可用</Caption1></div>
        <Switch id="startup-enabled" aria-label="登录 Windows 时自动启动" checked={s.startupEnabled} disabled={s.busy} onChange={(_, data) => s.setStartupEnabled(data.checked)} />
        {s.startupEnabled && <div className="startup-detail"><Caption1>启动方式</Caption1><Select id="startup-mode" value={s.startupMode} aria-label="启动方式" disabled={s.busy} onChange={(_, data) => s.setStartupMode(data.value as StartupMode)}>
          <option value="task">计划任务（推荐）</option><option value="run">兼容方式</option>
        </Select></div>}
      </div>
      <div className="preference-row">
        <span className="preference-icon"><Play20Regular /></span><div className="preference-copy"><Text weight="medium">安装完成后打开</Text><Caption1>准备好后，直接开始使用</Caption1></div>
        <Switch id="launch-after-install" aria-label="安装完成后打开 Glide-Dock" checked={s.launchAfterInstall} disabled={s.busy} onChange={(_, data) => s.setLaunchAfterInstall(data.checked)} />
      </div>
    </Card>
    <Accordion collapsible className="apps-accordion" openItems={s.appsOpen ? ['apps'] : []} onToggle={(_, data) => s.setAppsOpen(data.openItems.includes('apps'))}>
      <AccordionItem value="apps">
        <AccordionHeader id="apps-toggle" icon={<Apps20Regular />}><span className="apps-heading"><Text weight="medium">添加常用应用</Text><Caption1 id="app-scan-count">{s.selected.size ? '已选 ' + s.selected.size + ' 个' : '可选'}</Caption1></span></AccordionHeader>
        <AccordionPanel>
          <div className="apps-section" id="recommended-apps">
            <Caption1 className="muted" id="apps-heading">从本机开始菜单和桌面找到的应用中选择。</Caption1>
            <div className="apps-toolbar">
              <Input id="app-search" type="search" contentBefore={<Search16Regular />} placeholder="搜索应用" aria-label="搜索应用" value={s.query} disabled={s.busy} onChange={(_, data) => s.setQuery(data.value)} />
              <Button id="select-apps" appearance="subtle" size="small" disabled={s.busy || !s.visibleApps.length} onClick={s.selectVisible}>全选</Button>
              <Button id="clear-apps" appearance="subtle" size="small" disabled={s.busy || !s.selected.size} onClick={() => s.setSelected(new Set())}>清空</Button>
            </div>
            <div className="app-list" id="app-list" role="group" aria-labelledby="apps-heading">
              {s.visibleApps.map(({ app, index }) => <Checkbox key={index} className="app-row" checked={s.selected.has(index)} disabled={s.busy}
                aria-label={'添加 ' + app.name + ' 到 Dock'} onChange={(_, data) => s.toggleApp(index, !!data.checked)}
                label={<span className="app-row-copy" title={app.name + '\n' + app.path}><Text truncate wrap={false}>{app.name}</Text><Caption1 className="muted">{app.source}</Caption1></span>} />)}
              {!s.visibleApps.length && <Text className="empty-state muted">{s.info?.apps.length ? '没有匹配的应用。' : '没有找到可添加的应用，可以安装后再添加。'}</Text>}
            </div>
          </div>
        </AccordionPanel>
      </AccordionItem>
    </Accordion>
  </section>;
}

function EnvironmentChecks({ state }: { state: InstallerState }) {
  return <ul className="environment-checks">{state.info?.checks.map((check, index) => <li key={index}>
    <StatusIcon intent={check.passed ? 'success' : 'error'} /><Text size={200}><strong>{check.name}</strong>　{check.message}</Text>
  </li>)}</ul>;
}

function Confirmation({ state: s }: { state: InstallerState }) {
  const updating = s.mode === 'update';
  const apps = s.info?.apps.filter((_, index) => s.selected.has(index)) ?? [];
  const details = [
    ...(updating ? [['已安装版本', s.info?.existing?.version ?? '无法识别'], ['安装包版本', s.info?.version ?? '']] : []),
    ['安装到', s.installPath.trim()], ['安装范围', s.scope === 'all-users' ? '这台电脑的所有用户' : '当前账户'],
    ['颜色主题', updating ? '保留当前设置' : { system: '跟随系统', light: '浅色', dark: '深色' }[s.theme]],
    ['登录启动', updating ? '保留当前设置' : !s.startupEnabled ? '关闭' : s.startupMode === 'task' ? '计划任务' : '兼容方式'],
    ['桌面快捷方式', updating ? '保留现状' : s.desktopShortcut ? '创建' : '不创建'],
    ['加入 Dock', updating ? '保留现有条目' : apps.length ? apps.length + ' 个：' + apps.slice(0, 3).map(app => app.name).join('、') + (apps.length > 3 ? '…' : '') : '暂不添加'],
  ];
  return <section className="page" id="confirm-page">
    <Intro title={updating ? s.info?.update_eligibility?.title ?? '无法判断已安装版本' : '准备安装'}>{updating ? s.info?.update_eligibility?.message : '确认以下内容，准备好后即可开始安装。'}</Intro>
    <Card appearance="outline" className="summary-card"><dl className="install-summary">{details.map(([label, value]) => <div key={label}><dt>{label}</dt><dd id={label === '安装到' ? 'summary-path' : undefined}>{value}</dd></div>)}</dl></Card>
    <div className="preflight"><Subtitle2 as="h3">安装环境</Subtitle2><EnvironmentChecks state={s} /></div>
    {!s.info?.payload_ready && <MessageBar intent="error"><MessageBarBody>此构建未包含安装文件，请使用发布打包脚本生成的安装程序。</MessageBarBody></MessageBar>}
    {!s.checking && !s.destination.valid && s.destination.message && (!updating || s.info?.update_eligibility?.allowed) && <MessageBar intent="error"><MessageBarBody>{s.destination.message}</MessageBarBody></MessageBar>}
    {s.scope === 'all-users' && <Caption1 className="muted">此操作会请求 Windows 管理员确认。</Caption1>}
  </section>;
}

function AutomaticUpdatePage({ state: s }: { state: InstallerState }) {
  const checksFailed = s.info?.checks.some(check => !check.passed) ?? false;
  return <section className="page" id="auto-update-page">
    <Intro title={s.phase === 'complete' ? '更新已完成' : s.phase === 'failed' ? '更新未能开始' : '正在准备自动更新'}>
      {s.feedback.message}
    </Intro>
    <Card appearance="outline" className="summary-card">
      <dl className="install-summary">
        <div><dt>当前版本</dt><dd>{s.info?.existing?.version ?? '未知'}</dd></div>
        <div><dt>目标版本</dt><dd>{s.info?.version ?? ''}</dd></div>
        <div><dt>安装位置</dt><dd id="summary-path">{s.installPath}</dd></div>
        <div><dt>设置与 Dock 条目</dt><dd>保留</dd></div>
      </dl>
    </Card>
    {s.checking && <Spinner size="small" label="正在检查更新条件…" />}
    {!s.info?.payload_ready && <MessageBar intent="error"><MessageBarBody>安装包资源不完整，无法执行更新。请重新下载完整的安装包。</MessageBarBody></MessageBar>}
    {s.info?.update_eligibility && !s.info.update_eligibility.allowed && <MessageBar intent="error"><MessageBarBody>{s.info.update_eligibility.message}</MessageBarBody></MessageBar>}
    {!s.checking && !s.destination.valid && s.destination.message && <MessageBar intent="error"><MessageBarBody>{s.destination.message}</MessageBarBody></MessageBar>}
    {checksFailed && <div className="preflight"><EnvironmentChecks state={s} /></div>}
    {s.scope === 'all-users' && <Caption1 className="muted">此更新需要 Windows 管理员确认。</Caption1>}
  </section>;
}

function UninstallPage({ state: s }: { state: InstallerState }) {
  return <section className="page management-page" id="uninstall-view">
    <div className="manage-brand"><img src={brandMark} alt="" /><Badge appearance="tint" color="brand">已安装</Badge></div>
    <Intro title="卸载 Glide-Dock">{s.info?.existing?.install_directory}</Intro>
    <Text className="muted">卸载会移除程序文件和快捷方式，默认保留当前账户的设置、Dock 条目和日志。</Text>
    <div className="manage-actions"><Button id="uninstall-action" className="manage-action" icon={<Delete20Regular />} disabled={s.busy} onClick={() => s.setUninstallOpen(true)}>
      <span className="action-copy"><Text weight="semibold">卸载 Glide-Dock</Text><Caption1 className="muted">默认保留个人设置和日志</Caption1></span><ChevronRight16Regular />
    </Button></div>
  </section>;
}

function UninstallDialog({ state: s }: { state: InstallerState }) {
  return <Dialog open={s.uninstallOpen} onOpenChange={(_, data) => { if (!s.busy) s.setUninstallOpen(data.open); }}>
    <DialogSurface id="uninstall-dialog"><DialogBody>
      <DialogTitle>卸载 Glide-Dock</DialogTitle>
      <DialogContent className="uninstall-content">
        <Text>将移除程序文件。保留个人数据后，日后重新安装仍可继续使用原有配置。</Text>
        <Checkbox id="delete-personal-data" label="同时删除当前账户的设置、Dock 条目和日志" checked={s.deletePersonalData} disabled={s.busy} onChange={(_, data) => s.setDeletePersonalData(!!data.checked)} />
        {s.deletePersonalData && <MessageBar intent="warning"><MessageBarBody>个人数据删除后无法恢复。</MessageBarBody></MessageBar>}
      </DialogContent>
      <DialogActions><Button onClick={() => s.setUninstallOpen(false)} disabled={s.busy}>取消</Button><Button id="confirm-uninstall" appearance="primary" className="danger-action" disabled={s.busy} onClick={() => { void s.beginUninstall(); }}>确认卸载</Button></DialogActions>
    </DialogBody></DialogSurface>
  </Dialog>;
}

export default function App() {
  const state = useInstaller();
  const [dark, setDark] = useState(() => window.matchMedia('(prefers-color-scheme: dark)').matches);
  const content = useRef<HTMLElement>(null);
  useEffect(() => {
    const media = window.matchMedia('(prefers-color-scheme: dark)');
    const change = () => setDark(media.matches);
    media.addEventListener('change', change);
    return () => media.removeEventListener('change', change);
  }, []);
  const loaded = !!state.info;
  useEffect(() => {
    content.current?.scrollTo({ top: 0 });
    if (!state.uninstallOpen) content.current?.querySelector<HTMLElement>('h2')?.focus({ preventScroll: true });
  }, [state.page, state.mode, state.complete, state.uninstallOpen, loaded]);
  const showFeedbackDetails = !state.loadError && ['error', 'warning'].includes(state.feedback.intent);
  const footerMessage = showFeedbackDetails
    ? state.feedback.intent === 'error' ? '操作未完成，请查看详细信息后重试。' : '请查看详细提示。'
    : state.feedback.message;
  const completionTitle = state.mode === 'uninstall' ? '卸载已完成' : state.mode === 'update' ? '更新已完成' : '安装已完成';
  return <FluentProvider theme={{ ...(dark ? webDarkTheme : webLightTheme), fontFamilyBase: '"Segoe UI Variable", "Segoe UI", "Microsoft YaHei UI", "Microsoft YaHei", sans-serif', fontFamilyNumeric: '"Segoe UI Variable", "Segoe UI", sans-serif' }} applyStylesToPortals={false} className="setup-provider" data-theme={dark ? 'dark' : 'light'}>
    <div className="setup-window">
      <Titlebar state={state} />
      <div className="setup-body">
        <Sidebar state={state} />
        <div className="workspace">
      <main ref={content} className="content" id="main-content" tabIndex={0} aria-busy={state.busy}>
        {state.loadError ? <section className="page"><Intro title="暂时无法继续">安装信息未能加载。</Intro><MessageBar intent="error"><MessageBarBody>{state.loadError}</MessageBarBody></MessageBar><Button onClick={() => window.location.reload()}>重新检查</Button></section>
          : !state.info ? <div className="loading-state"><Spinner size="small" label="正在检查安装信息…" /></div>
          : state.complete ? <section className="page completion-page" id="completion-view"><CheckmarkCircle32Regular className="completion-icon" /><Intro title={completionTitle}>{state.mode === 'uninstall' ? 'Glide-Dock 程序文件已移除。' : 'Glide-Dock 已准备就绪。之后可以从桌面或开始菜单打开。'}</Intro></section>
          : state.mode === 'uninstall' ? <UninstallPage state={state} />
          : state.mode === 'update' && state.info.auto_update ? <AutomaticUpdatePage state={state} />
          : state.mode === 'update' || state.page === 'confirm' ? <Confirmation state={state} />
          : state.page === 'setup' ? <Preferences state={state} /> : <Location state={state} />}
        {showFeedbackDetails && <div className="feedback-details"><MessageBar intent={state.feedback.intent === 'error' ? 'error' : 'warning'}><MessageBarBody>{state.feedback.message}</MessageBarBody></MessageBar></div>}
      </main>
      <Divider className="footer-divider" />
      <footer className="footer">
        <div className="footer-status" role="status" aria-live="polite">
          <div className="status-line"><StatusIcon intent={state.feedback.intent} /><Text size={200} id="operation-message" title={state.feedback.message}>{footerMessage}</Text></div>
          {state.progress !== null && <ProgressBar id="progress-track" value={state.progress / 100} thickness="medium" color={state.phase === 'failed' ? 'error' : state.complete ? 'success' : undefined} aria-label="操作进度" />}
        </div>
        <div className="footer-actions">
          {state.mode === 'install' && state.page !== 'location' && !state.complete && <Button id="back-button" icon={<ArrowLeft16Regular />} disabled={state.busy} onClick={() => state.navigate(state.page === 'confirm' ? 'setup' : 'location')}>返回</Button>}
          <Button id="primary-button" appearance="primary" disabled={state.primaryDisabled} onClick={state.primaryAction}
            icon={!state.complete && !state.busy && ['下一步', '查看安装清单'].includes(state.primaryLabel) ? <ArrowRight16Regular /> : undefined} iconPosition="after">{state.primaryLabel}</Button>
        </div>
      </footer>
        </div>
      </div>
      <UninstallDialog state={state} />
    </div>
  </FluentProvider>;
}
