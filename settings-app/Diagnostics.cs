using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using Microsoft.Win32;

namespace GlideDock.Settings;

internal sealed record DiagnosticsSnapshot(
    string Summary,
    bool HasWarning,
    IReadOnlyList<(string Label, string Value)> Rows,
    string PlainText);

internal readonly record struct ProcessIdentity(
    bool Running,
    uint Pid,
    string Integrity,
    bool Elevated);

/// <summary>
/// Read-only environment checks for the settings diagnostics page. It never
/// changes the Dock or the registry.
/// </summary>
internal static class DiagnosticsService
{
    private const uint ProcessQueryLimitedInformation = 0x1000;
    private const uint TokenQuery = 0x0008;
    private const uint Synchronize = 0x00100000;
    private const int TokenElevationClass = 20;
    private const int TokenIntegrityClass = 25;
    private const string DockWindowClass = "Glide-Dock.DockWindow";
    private const string GuardMutexName = "Local\\Glide-Dock.Guard";

    public static DiagnosticsSnapshot Collect(string configPath, StartupMode preferredMode)
    {
        var dock = QueryWindow(DockWindowClass);
        var settings = QueryProcess((uint)Environment.ProcessId);
        var explorer = QueryProcess(GetShellPid());
        var guardRunning = GuardRunning();
        var uac = ReadUac();
        var startup = ReadStartup(preferredMode);
        var compatibility = ReadCompatibility();
        var shortcut = ReadShortcut();
        var logDirectory = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "Glide-Dock",
            "logs");

        var rows = new List<(string, string)>
        {
            ("Dock", Describe(dock)),
            ("设置中心", Describe(settings)),
            ("资源管理器", Describe(explorer)),
            ("UAC", uac),
            ("守护进程", guardRunning ? "运行中（Local\\Glide-Dock.Guard）" : "未运行"),
            ("启动方式", startup),
            ("开始菜单快捷方式", shortcut),
            ("兼容性标记", compatibility),
            ("配置文件", configPath),
            ("Dock 路径", string.IsNullOrWhiteSpace(App.DockExecutablePath) ? "未定位" : App.DockExecutablePath),
            ("日志目录", logDirectory)
        };

        var problem = new List<string>();
        if (settings.Elevated)
        {
            problem.Add("设置中心以管理员权限运行");
        }
        if (dock.Running && dock.Elevated)
        {
            problem.Add("Dock 以管理员权限运行，拖放会被 UIPI 拦截");
        }
        if (dock.Running && dock.Integrity == "Low")
        {
            problem.Add("Dock 处于 Low Integrity");
        }
        if (!dock.Running)
        {
            problem.Add("Dock 未运行");
        }
        if (explorer.Running && explorer.Integrity == "Low")
        {
            problem.Add("Explorer 处于 Low Integrity");
        }

        var summary = problem.Count == 0
            ? "环境正常"
            : string.Join("；", problem);
        var warning = problem.Count > 0;
        var text = new StringBuilder();
        text.AppendLine("Glide-Dock 诊断");
        text.AppendLine(summary);
        text.AppendLine();
        foreach (var (label, value) in rows)
        {
            text.Append(label).Append("：").AppendLine(value);
        }
        return new DiagnosticsSnapshot(summary, warning, rows, text.ToString());
    }

    private static string Describe(ProcessIdentity identity)
    {
        if (!identity.Running)
        {
            return identity.Pid == 0 ? "未运行" : "无法读取";
        }
        var elevation = identity.Elevated ? "管理员" : "普通用户";
        return $"运行中（PID {identity.Pid}，{elevation}，{identity.Integrity}）";
    }

    private static ProcessIdentity QueryWindow(string className)
    {
        var hwnd = FindWindowW(className, null);
        if (hwnd == IntPtr.Zero)
        {
            return new ProcessIdentity(false, 0, "未知", false);
        }
        GetWindowThreadProcessId(hwnd, out var pid);
        return pid == 0
            ? new ProcessIdentity(false, 0, "未知", false)
            : QueryProcess(pid);
    }

    private static ProcessIdentity QueryProcess(uint pid)
    {
        if (pid == 0)
        {
            return new ProcessIdentity(false, 0, "未知", false);
        }

        var process = OpenProcess(ProcessQueryLimitedInformation, false, pid);
        if (process == IntPtr.Zero)
        {
            return new ProcessIdentity(true, pid, "未知", false);
        }

        IntPtr token = IntPtr.Zero;
        try
        {
            if (!OpenProcessToken(process, TokenQuery, out token))
            {
                return new ProcessIdentity(true, pid, "未知", false);
            }

            var elevated = ReadElevation(token);
            var integrity = ReadIntegrity(token);
            return new ProcessIdentity(true, pid, integrity, elevated);
        }
        finally
        {
            if (token != IntPtr.Zero)
            {
                CloseHandle(token);
            }
            CloseHandle(process);
        }
    }

    private static bool ReadElevation(IntPtr token)
    {
        var size = Marshal.SizeOf<TOKEN_ELEVATION>();
        var buffer = Marshal.AllocHGlobal(size);
        try
        {
            if (!GetTokenInformation(token, TokenElevationClass, buffer, (uint)size, out _))
            {
                return false;
            }
            return Marshal.PtrToStructure<TOKEN_ELEVATION>(buffer).TokenIsElevated != 0;
        }
        finally
        {
            Marshal.FreeHGlobal(buffer);
        }
    }

    private static string ReadIntegrity(IntPtr token)
    {
        const int bufferSize = 256;
        var buffer = Marshal.AllocHGlobal(bufferSize);
        try
        {
            if (!GetTokenInformation(token, TokenIntegrityClass, buffer, bufferSize, out _))
            {
                return "未知";
            }
            var label = Marshal.PtrToStructure<TOKEN_MANDATORY_LABEL>(buffer);
            var countPointer = GetSidSubAuthorityCount(label.Label.Sid);
            if (countPointer == IntPtr.Zero)
            {
                return "未知";
            }
            var count = Marshal.ReadByte(countPointer);
            if (count == 0)
            {
                return "未知";
            }
            var ridPointer = GetSidSubAuthority(label.Label.Sid, (uint)(count - 1));
            if (ridPointer == IntPtr.Zero)
            {
                return "未知";
            }
            var rid = Marshal.ReadInt32(ridPointer);
            return rid switch
            {
                < 0x2000 => "Low",
                < 0x3000 => "Medium",
                < 0x4000 => "High",
                _ => "System"
            };
        }
        finally
        {
            Marshal.FreeHGlobal(buffer);
        }
    }

    private static uint GetShellPid()
    {
        var shell = GetShellWindow();
        if (shell == IntPtr.Zero)
        {
            return 0;
        }
        GetWindowThreadProcessId(shell, out var pid);
        return pid;
    }

    private static bool GuardRunning()
    {
        var mutex = OpenMutexW(Synchronize, false, GuardMutexName);
        if (mutex == IntPtr.Zero)
        {
            return false;
        }
        CloseHandle(mutex);
        return true;
    }

    private static string ReadUac()
    {
        try
        {
            using var key = Registry.LocalMachine.OpenSubKey(
                @"SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System");
            if (key is null)
            {
                return "无法读取";
            }
            var enableLua = key.GetValue("EnableLUA");
            var prompt = key.GetValue("ConsentPromptBehaviorAdmin");
            var secure = key.GetValue("PromptOnSecureDesktop");
            return $"EnableLUA={enableLua ?? "?"}，ConsentPromptBehaviorAdmin={prompt ?? "?"}，PromptOnSecureDesktop={secure ?? "?"}";
        }
        catch (Exception error)
        {
            return "无法读取：" + error.Message;
        }
    }

    private static string ReadStartup(StartupMode preferredMode)
    {
        try
        {
            var task = StartupTask.Query();
            var run = StartupRun.TryRead(out var runCommand);
            if (task.Exists && task.Enabled && run)
            {
                return "快速（计划任务）和兼容（HKCU Run）同时存在";
            }
            if (task.Exists && task.Enabled)
            {
                return "快速（计划任务，已启用）";
            }
            if (run)
            {
                return "兼容（HKCU Run，已启用）";
            }
            return preferredMode == StartupMode.Run
                ? "未启用（偏好：兼容 Run）"
                : "未启用（偏好：快速计划任务）";
        }
        catch (Exception error)
        {
            return "无法读取：" + error.Message;
        }
    }

    private static string ReadCompatibility()
    {
        if (string.IsNullOrWhiteSpace(App.DockExecutablePath))
        {
            return "未定位 Dock 路径";
        }
        try
        {
            using var key = Registry.CurrentUser.OpenSubKey(
                @"Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Layers");
            var value = key?.GetValue(App.DockExecutablePath) as string;
            return string.IsNullOrWhiteSpace(value) ? "未发现兼容性标记" : value;
        }
        catch (Exception error)
        {
            return "无法读取：" + error.Message;
        }
    }

    private static string ReadShortcut()
    {
        try
        {
            var paths = new[]
            {
                Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.CommonStartMenu),
                    "Programs", "Glide-Dock", "Glide-Dock.lnk"),
                Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
                    "Microsoft", "Windows", "Start Menu", "Programs", "Glide-Dock", "Glide-Dock.lnk")
            };
            foreach (var path in paths)
            {
                if (File.Exists(path))
                {
                    return "已找到：" + path;
                }
            }
            return "未找到 Glide-Dock.lnk";
        }
        catch (Exception error)
        {
            return "无法读取：" + error.Message;
        }
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct TOKEN_ELEVATION
    {
        public int TokenIsElevated;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct SID_AND_ATTRIBUTES
    {
        public IntPtr Sid;
        public uint Attributes;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct TOKEN_MANDATORY_LABEL
    {
        public SID_AND_ATTRIBUTES Label;
    }

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern IntPtr FindWindowW(string className, string? windowName);

    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint processId);

    [DllImport("user32.dll")]
    private static extern IntPtr GetShellWindow();

    [DllImport("kernel32.dll")]
    private static extern IntPtr OpenProcess(uint access, bool inherit, uint processId);

    [DllImport("advapi32.dll")]
    private static extern bool OpenProcessToken(IntPtr process, uint access, out IntPtr token);

    [DllImport("advapi32.dll")]
    private static extern bool GetTokenInformation(
        IntPtr token, int infoClass, IntPtr info, uint length, out uint needed);

    [DllImport("advapi32.dll")]
    private static extern IntPtr GetSidSubAuthorityCount(IntPtr sid);

    [DllImport("advapi32.dll")]
    private static extern IntPtr GetSidSubAuthority(IntPtr sid, uint index);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    private static extern IntPtr OpenMutexW(uint access, bool inherit, string name);

    [DllImport("kernel32.dll")]
    private static extern bool CloseHandle(IntPtr handle);
}
