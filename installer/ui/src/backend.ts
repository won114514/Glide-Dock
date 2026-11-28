export type Mode = 'install' | 'update' | 'uninstall';
export type Scope = 'current-user' | 'all-users';
export type Theme = 'system' | 'light' | 'dark';
export type StartupMode = 'task' | 'run';
export type Page = 'location' | 'setup' | 'confirm';
export interface RecommendedApp { name: string; path: string; source: string }
export interface PreflightCheck { name: string; message: string; passed: boolean }
export interface InstallRecord { scope: Scope; install_directory: string; version: string | null }
export interface UpdateEligibility { allowed: boolean; title: string; message: string }
export interface SetupInfo {
  version: string;
  mode: Mode;
  auto_update: boolean;
  payload_ready: boolean;
  update_eligibility: UpdateEligibility | null;
  install_directory: string;
  existing: InstallRecord | null;
  checks: PreflightCheck[];
  apps: RecommendedApp[];
}
export interface DestinationCheck { valid: boolean; message: string; space: string }
export interface OperationStatus { percent: number; message: string; phase: 'running' | 'complete' | 'failed'; warnings?: string[] }
interface SetupWindow {
  close(): Promise<void>;
  onCloseRequested(handler: (event: { preventDefault(): void }) => void): Promise<() => void>;
}
declare global {
  interface Window {
    __TAURI__: {
      core: { invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> };
      window: { getCurrentWindow(): SetupWindow };
    };
  }
}
export const invoke = <T,>(command: string, args?: Record<string, unknown>) => window.__TAURI__.core.invoke<T>(command, args);
export const setupWindow = window.__TAURI__.window.getCurrentWindow();
