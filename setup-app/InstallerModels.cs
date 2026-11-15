using System.Text;
using System.Text.Json;

namespace GlideDock.Setup;

internal enum InstallScope
{
    CurrentUser,
    AllUsers
}

internal sealed record InstallOptions
{
    public InstallScope Scope { get; init; } = InstallScope.AllUsers;
    public string InstallDirectory { get; init; } = string.Empty;
    public bool CreateDesktopShortcut { get; init; }
    public bool EnableStartup { get; init; }
    public bool LaunchAfterInstall { get; init; } = true;
    public string Theme { get; init; } = "system";
    public List<string> SelectedApps { get; init; } = [];
    public string OwnerSid { get; init; } = string.Empty;
    public bool IsUpdate { get; init; }

    public string ToBase64() => Convert.ToBase64String(
        Encoding.UTF8.GetBytes(JsonSerializer.Serialize(this)));

    public static InstallOptions FromBase64(string value) =>
        JsonSerializer.Deserialize<InstallOptions>(Encoding.UTF8.GetString(Convert.FromBase64String(value)))
        ?? throw new InvalidDataException("安装参数无效。");
}

internal sealed record UninstallRecord(
    InstallScope Scope,
    string InstallDirectory,
    string Version,
    string OwnerSid,
    bool IsLegacy = false)
{
    public string ToBase64() => Convert.ToBase64String(
        Encoding.UTF8.GetBytes(JsonSerializer.Serialize(this)));

    public static UninstallRecord FromBase64(string value) =>
        JsonSerializer.Deserialize<UninstallRecord>(Encoding.UTF8.GetString(Convert.FromBase64String(value)))
        ?? throw new InvalidDataException("卸载参数无效。");
}

internal sealed record AppCandidate(string Id, string Name, string Path)
{
    public static IReadOnlyList<AppCandidate> Discover()
    {
        var windows = Environment.GetFolderPath(Environment.SpecialFolder.Windows);
        var local = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
        var candidates = new (string Id, string Name, string[] Paths)[]
        {
            ("explorer", "文件资源管理器", [System.IO.Path.Combine(windows, "explorer.exe")]),
            ("edge", "Microsoft Edge", [
                @"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
                @"C:\Program Files\Microsoft\Edge\Application\msedge.exe"]),
            ("terminal", "终端", [
                System.IO.Path.Combine(local, "Microsoft", "WindowsApps", "wt.exe"),
                System.IO.Path.Combine(windows, "System32", "cmd.exe")]),
            ("powershell", "PowerShell", [System.IO.Path.Combine(windows, "System32", "WindowsPowerShell", "v1.0", "powershell.exe")]),
            ("calculator", "计算器", [System.IO.Path.Combine(windows, "System32", "calc.exe")]),
            ("notepad", "记事本", [System.IO.Path.Combine(windows, "System32", "notepad.exe")]),
            ("taskmgr", "任务管理器", [System.IO.Path.Combine(windows, "System32", "Taskmgr.exe")]),
            ("settings", "设置", [
                System.IO.Path.Combine(windows, "ImmersiveControlPanel", "SystemSettings.exe"),
                System.IO.Path.Combine(windows, "System32", "control.exe")])
        };

        return candidates
            .Select(candidate => (candidate.Id, candidate.Name,
                Path: candidate.Paths.FirstOrDefault(File.Exists)))
            .Where(candidate => candidate.Path is not null)
            .Select(candidate => new AppCandidate(candidate.Id, candidate.Name, candidate.Path!))
            .ToArray();
    }
}
