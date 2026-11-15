using System;
using System.IO;
using Microsoft.Win32;

namespace GlideDock.Settings;

/// <summary>
/// Per-user HKCU Run registration used as the non-elevated compatibility mode.
/// </summary>
internal static class StartupRun
{
    private const string RunKeyPath = @"Software\Microsoft\Windows\CurrentVersion\Run";
    private const string ValueName = "Glide-Dock";

    public static bool TryRead(out string command)
    {
        command = string.Empty;
        using var key = Registry.CurrentUser.OpenSubKey(RunKeyPath, writable: false);
        if (key?.GetValue(ValueName) is not string value || string.IsNullOrWhiteSpace(value))
        {
            return false;
        }

        command = value;
        return true;
    }

    public static string? RegisteredExecutable(string command)
    {
        var trimmed = command.Trim();
        if (trimmed.Length == 0)
        {
            return null;
        }

        if (trimmed[0] == '"')
        {
            var end = trimmed.IndexOf('"', 1);
            return end > 1 ? trimmed[1..end] : null;
        }

        var space = trimmed.IndexOf(' ');
        return space < 0 ? trimmed : trimmed[..space];
    }

    public static void Enable(string executablePath)
    {
        var path = ValidateExecutable(executablePath);
        using var key = Registry.CurrentUser.CreateSubKey(RunKeyPath, writable: true)
            ?? throw new IOException("无法打开当前用户的 Run 注册表项。");
        key.SetValue(ValueName, Quote(path), RegistryValueKind.String);
    }

    public static void Disable()
    {
        using var key = Registry.CurrentUser.OpenSubKey(RunKeyPath, writable: true);
        key?.DeleteValue(ValueName, throwOnMissingValue: false);
    }

    public static string ValidateExecutable(string executablePath)
    {
        var path = Path.GetFullPath(executablePath);
        if (!File.Exists(path))
        {
            throw new FileNotFoundException("找不到 Glide-Dock 可执行文件。", path);
        }
        if (!string.Equals(Path.GetExtension(path), ".exe", StringComparison.OrdinalIgnoreCase))
        {
            throw new InvalidDataException("启动项只接受 .exe 文件。");
        }
        if (path.Contains('"', StringComparison.Ordinal))
        {
            throw new InvalidDataException("可执行文件路径不能包含双引号。");
        }
        return path;
    }

    public static string Quote(string path) => "\"" + path + "\"";
}
