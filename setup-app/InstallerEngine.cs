using System.Diagnostics;
using System.IO.Compression;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Security.Principal;
using System.Text.Json.Nodes;
using Microsoft.Win32;

namespace GlideDock.Setup;

internal static class InstallerEngine
{
    private const string ProductName = "Glide-Dock";
    private const string OldInnoAppId = "{9C6F1D7A-4B23-4D8E-9C1A-6E2F0A7B3C51}_is1";
    private const string UninstallKeyPath = @"Software\Microsoft\Windows\CurrentVersion\Uninstall\Glide-Dock";
    private const string OldUninstallKeyPath = @"Software\Microsoft\Windows\CurrentVersion\Uninstall\" + OldInnoAppId;
    private const string RunKeyPath = @"Software\Microsoft\Windows\CurrentVersion\Run";
    private const string StartupTaskName = "Glide-Dock";
    private const string SetupExecutable = "GlideDock.Setup.exe";
    private const string PayloadResource = "GlideDock.Setup.Payload.zip";

    public static string DefaultInstallDirectory(InstallScope scope) => scope switch
    {
        InstallScope.AllUsers => Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), ProductName),
        _ => Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "Programs", ProductName)
    };

    public static string CurrentOwnerSid()
    {
        try
        {
            return WindowsIdentity.GetCurrent().User?.Value ?? string.Empty;
        }
        catch
        {
            return string.Empty;
        }
    }

    public static IReadOnlyList<EnvironmentCheck> CheckEnvironment(string installDirectory)
    {
        var checks = new List<EnvironmentCheck>();
        var supportedArchitecture = RuntimeInformation.OSArchitecture == Architecture.X64;
        var supportedWindows = OperatingSystem.IsWindowsVersionAtLeast(10, 0, 17763);
        checks.Add(new EnvironmentCheck(
            "系统兼容性",
            supportedArchitecture && supportedWindows
                ? "Windows 10 1809 或更高版本，64 位系统。"
                : "需要 Windows 10 1809 或更高版本的 64 位系统。",
            supportedArchitecture && supportedWindows ? CheckState.Passed : CheckState.Failed));

        try
        {
            var payload = InspectPayload();
            checks.Add(new EnvironmentCheck(
                "安装包完整性",
                $"程序文件可读取，共 {payload.FileCount} 个文件。",
                CheckState.Passed));

            try
            {
                var available = GetAvailableSpace(installDirectory);
                var required = RequiredInstallSpace(payload.UncompressedBytes);
                var enough = available >= required;
                checks.Add(new EnvironmentCheck(
                    "默认位置空间",
                    $"可用 {FormatBytes(available)}，建议至少 {FormatBytes(required)}。" +
                    (enough ? string.Empty : " 可在安装选项中更换磁盘。"),
                    enough ? CheckState.Passed : CheckState.Warning));
            }
            catch (Exception error)
            {
                checks.Add(new EnvironmentCheck(
                    "默认位置空间",
                    "无法读取目标磁盘空间；选择安装位置时会再次检查。" + error.Message,
                    CheckState.Warning));
            }
        }
        catch (Exception error)
        {
            checks.Add(new EnvironmentCheck("安装包完整性", error.Message, CheckState.Failed));
            checks.Add(new EnvironmentCheck(
                "默认位置空间",
                "安装包无法读取，暂时无法计算所需空间。",
                CheckState.Warning));
        }

        return checks;
    }

    public static void ValidateInstallCapacity(string installDirectory)
    {
        var payload = InspectPayload();
        var required = RequiredInstallSpace(payload.UncompressedBytes);
        var available = GetAvailableSpace(installDirectory);
        if (available < required)
        {
            throw new IOException(
                $"所选磁盘空间不足。可用 {FormatBytes(available)}，建议至少 {FormatBytes(required)}。请选择其他位置。 ");
        }
    }

    public static UninstallRecord? FindInstall()
    {
        var machine = ReadInstallRecord(RegistryHive.LocalMachine, InstallScope.AllUsers)
            ?? ReadLegacyRecord(RegistryHive.LocalMachine, InstallScope.AllUsers);
        if (machine is not null)
        {
            return machine;
        }

        return ReadInstallRecord(RegistryHive.CurrentUser, InstallScope.CurrentUser)
            ?? ReadLegacyRecord(RegistryHive.CurrentUser, InstallScope.CurrentUser);
    }

    public static bool HasStartupRegistration()
    {
        try
        {
            using var key = Registry.CurrentUser.OpenSubKey(RunKeyPath);
            if (key?.GetValue(ProductName) is string command && !string.IsNullOrWhiteSpace(command))
            {
                return true;
            }
        }
        catch
        {
            // Querying startup is best-effort; setup remains available.
        }

        try
        {
            using var process = Process.Start(new ProcessStartInfo
            {
                FileName = "schtasks.exe",
                UseShellExecute = false,
                CreateNoWindow = true,
                ArgumentList = { "/Query", "/TN", StartupTaskName }
            });
            process?.WaitForExit(5000);
            return process?.ExitCode == 0;
        }
        catch
        {
            return false;
        }
    }

    public static async Task InstallAsync(
        InstallOptions options,
        string setupExecutable,
        bool machineScope,
        CancellationToken cancellationToken,
        IProgress<InstallProgress>? progress = null)
    {
        await Task.Run(
            () => InstallCore(options, setupExecutable, machineScope, cancellationToken, progress),
            cancellationToken);
    }

    public static bool IsDockRunning() => Process.GetProcessesByName("glide-dock").Length > 0;

    public static void ConfigureUserProfile(InstallOptions options)
    {
        var configPath = ConfigPath;
        Directory.CreateDirectory(Path.GetDirectoryName(configPath)!);
        if (!File.Exists(configPath))
        {
            File.WriteAllText(configPath, CreateConfig(options), new System.Text.UTF8Encoding(false));
        }

        if (options.EnableStartup)
        {
            var executable = Path.Combine(options.InstallDirectory, "glide-dock.exe");
            using var key = Registry.CurrentUser.CreateSubKey(RunKeyPath, writable: true)
                ?? throw new IOException("无法创建当前用户的开机启动项。");
            key.SetValue(ProductName, $"\"{executable}\" --autostart", RegistryValueKind.String);
        }
        else
        {
            using var key = Registry.CurrentUser.OpenSubKey(RunKeyPath, writable: true);
            key?.DeleteValue(ProductName, throwOnMissingValue: false);
            TryRemoveStartupTask();
        }
    }

    public static async Task UninstallAsync(
        UninstallRecord record,
        int parentProcessId,
        IProgress<string>? progress,
        CancellationToken cancellationToken)
    {
        await Task.Run(() =>
        {
            WaitForParent(parentProcessId);
            progress?.Report("正在关闭 Glide-Dock...");
            StopProductProcesses();
            cancellationToken.ThrowIfCancellationRequested();

            progress?.Report("正在移除启动项和快捷方式...");
            RemoveStartup(record.OwnerSid, record.InstallDirectory);
            RemoveShortcuts(record.Scope);

            progress?.Report("正在清理安装信息...");
            RemoveRegistryRegistration(record.Scope);

            progress?.Report("正在移除程序文件...");
            DeleteInstallDirectory(record.InstallDirectory);
        }, cancellationToken);
    }

    public static void ValidateInstallDirectory(string directory, bool update)
    {
        if (string.IsNullOrWhiteSpace(directory))
        {
            throw new InvalidDataException("请选择安装位置。");
        }

        var fullPath = Path.GetFullPath(directory.Trim());
        var root = Path.GetPathRoot(fullPath);
        if (string.Equals(fullPath.TrimEnd(Path.DirectorySeparatorChar),
                root?.TrimEnd(Path.DirectorySeparatorChar), StringComparison.OrdinalIgnoreCase))
        {
            throw new InvalidDataException("不能将 Glide-Dock 安装到磁盘根目录。");
        }

        var windows = Environment.GetFolderPath(Environment.SpecialFolder.Windows);
        if (string.Equals(fullPath, windows, StringComparison.OrdinalIgnoreCase))
        {
            throw new InvalidDataException("不能将 Glide-Dock 安装到 Windows 系统目录。");
        }

        if (Directory.Exists(fullPath) && !update
            && Directory.EnumerateFileSystemEntries(fullPath).Any())
        {
            throw new InvalidDataException("所选文件夹不是空文件夹，无法安全安装到此处。");
        }
    }

    private static void InstallCore(
        InstallOptions options,
        string setupExecutable,
        bool machineScope,
        CancellationToken cancellationToken,
        IProgress<InstallProgress>? progress)
    {
        Report(progress, 1, "正在验证安装位置...");
        ValidateInstallDirectory(options.InstallDirectory, options.IsUpdate);
        var installDirectory = Path.GetFullPath(options.InstallDirectory);
        var parentDirectory = Path.GetDirectoryName(installDirectory)
            ?? throw new InvalidDataException("安装目录无效。");
        Report(progress, 3, "正在准备临时安装目录...");
        Directory.CreateDirectory(parentDirectory);

        var token = Guid.NewGuid().ToString("N");
        var stagingDirectory = installDirectory + ".stage-" + token;
        var backupDirectory = installDirectory + ".backup-" + token;
        var backupMoved = false;
        var newMoved = false;
        var registrationHive = machineScope ? RegistryHive.LocalMachine : RegistryHive.CurrentUser;
        var previousRegistration = ReadRegistrationValues(registrationHive);

        try
        {
            try
            {
                Directory.CreateDirectory(stagingDirectory);
                ExtractPayload(stagingDirectory, progress);
                cancellationToken.ThrowIfCancellationRequested();
                Report(progress, 82, "正在关闭 Glide-Dock...");
                StopProductProcesses();

                Report(progress, 87, "正在切换程序文件...");
                if (Directory.Exists(installDirectory))
                {
                    Directory.Move(installDirectory, backupDirectory);
                    backupMoved = true;
                }

                Directory.Move(stagingDirectory, installDirectory);
                newMoved = true;
                Report(progress, 93, "正在复制安装程序并创建快捷方式...");
                File.Copy(setupExecutable, Path.Combine(installDirectory, SetupExecutable), overwrite: true);
                CreateShortcuts(options, installDirectory, machineScope);
                Report(progress, 97, "正在写入安装信息...");
                WriteInstallRegistration(options, installDirectory, machineScope);
            }
            catch
            {
                RestoreRegistration(registrationHive, previousRegistration);
                if (newMoved && Directory.Exists(installDirectory))
                {
                    DeleteInstallDirectory(installDirectory);
                }
                if (backupMoved && Directory.Exists(backupDirectory) && !Directory.Exists(installDirectory))
                {
                    Directory.Move(backupDirectory, installDirectory);
                }
                throw;
            }

            RemoveLegacyRegistration();
            if (backupMoved)
            {
                try
                {
                    DeleteInstallDirectory(backupDirectory);
                }
                catch
                {
                    // The committed installation is valid; leave a backup if cleanup is blocked.
                }
            }
            Report(progress, 100, "安装完成。");
        }
        finally
        {
            if (Directory.Exists(stagingDirectory))
            {
                DeleteInstallDirectory(stagingDirectory);
            }
        }
    }

    private static void ExtractPayload(string destination, IProgress<InstallProgress>? progress)
    {
        using var archiveStream = Assembly.GetExecutingAssembly().GetManifestResourceStream(PayloadResource)
            ?? throw new InvalidOperationException("安装程序未包含 Glide-Dock 程序包载荷。");
        using var archive = new ZipArchive(archiveStream, ZipArchiveMode.Read);
        var entries = archive.Entries;
        var totalBytes = entries.Sum(entry => entry.Length);
        var completedBytes = 0L;
        var completedFiles = 0;
        var root = Path.GetFullPath(destination).TrimEnd(Path.DirectorySeparatorChar)
            + Path.DirectorySeparatorChar;
        var buffer = new byte[64 * 1024];

        foreach (var entry in entries)
        {
            var relativePath = entry.FullName.Replace('/', Path.DirectorySeparatorChar)
                .Replace('\\', Path.DirectorySeparatorChar);
            var targetPath = Path.GetFullPath(Path.Combine(destination, relativePath));
            if (!targetPath.StartsWith(root, StringComparison.OrdinalIgnoreCase))
            {
                throw new InvalidDataException("安装包包含无效的文件路径。");
            }

            if (entry.FullName.EndsWith('/') || entry.FullName.EndsWith('\\'))
            {
                Directory.CreateDirectory(targetPath);
                continue;
            }

            Directory.CreateDirectory(Path.GetDirectoryName(targetPath)!);
            using var source = entry.Open();
            using var destinationStream = new FileStream(
                targetPath, FileMode.CreateNew, FileAccess.Write, FileShare.None, buffer.Length, useAsync: false);
            int read;
            while ((read = source.Read(buffer, 0, buffer.Length)) > 0)
            {
                destinationStream.Write(buffer, 0, read);
                completedBytes += read;
                var percent = totalBytes == 0
                    ? 80
                    : 5 + (int)(75L * completedBytes / totalBytes);
                Report(progress, Math.Clamp(percent, 5, 80), $"正在解压程序文件（{completedFiles + 1}/{entries.Count}）...");
            }
            completedFiles++;
        }

        Report(progress, 80, "程序文件已释放，正在完成安装...");
    }

    private static (int FileCount, long UncompressedBytes) InspectPayload()
    {
        using var archiveStream = Assembly.GetExecutingAssembly().GetManifestResourceStream(PayloadResource)
            ?? throw new InvalidOperationException("安装程序未包含程序文件，请重新下载完整安装包。");
        using var archive = new ZipArchive(archiveStream, ZipArchiveMode.Read);
        var files = archive.Entries
            .Where(entry => !entry.FullName.EndsWith('/') && !entry.FullName.EndsWith('\\'))
            .ToArray();
        var names = files.Select(entry => entry.FullName.Replace('\\', '/'))
            .ToHashSet(StringComparer.OrdinalIgnoreCase);
        if (!names.Contains("glide-dock.exe") || !names.Contains("settings/GlideDock.Settings.exe"))
        {
            throw new InvalidDataException("安装包缺少 Glide-Dock 主程序或设置中心，请重新下载。");
        }

        return (files.Length, files.Sum(entry => entry.Length));
    }

    private static long RequiredInstallSpace(long uncompressedBytes) =>
        Math.Max(256L * 1024 * 1024, uncompressedBytes + 128L * 1024 * 1024);

    private static long GetAvailableSpace(string directory)
    {
        var fullPath = Path.GetFullPath(directory);
        var root = Path.GetPathRoot(fullPath);
        if (string.IsNullOrWhiteSpace(root))
        {
            throw new IOException("无法识别安装磁盘。");
        }
        return new DriveInfo(root).AvailableFreeSpace;
    }

    private static string FormatBytes(long bytes) =>
        $"{bytes / (1024d * 1024 * 1024):0.0} GB";

    private static void Report(IProgress<InstallProgress>? progress, int percent, string message) =>
        progress?.Report(new InstallProgress(percent, message));

    private static string CreateConfig(InstallOptions options)
    {
        var apps = AppCandidate.Discover().Where(app => options.SelectedApps.Contains(app.Id)).ToArray();
        var root = new JsonObject
        {
            ["version"] = 3,
            ["appearance"] = new JsonObject
            {
                ["icon_size"] = 48.0,
                ["spacing"] = 18.0,
                ["padding"] = 14.0,
                ["corner_radius"] = 20.0,
                ["opacity"] = 0.78,
                ["theme"] = options.Theme,
                ["accent"] = "blue",
                ["position"] = "bottom",
                ["layer"] = "desktop",
                ["edge_margin"] = 10.0,
                ["frosted_glass"] = true
            },
            ["behavior"] = new JsonObject
            {
                ["magnification"] = true,
                ["auto_hide"] = false,
                ["auto_hide_delay_ms"] = 450,
                ["launch_animation"] = true,
                ["show_tooltips"] = true,
                ["show_running_indicator"] = true
            },
            ["motion"] = new JsonObject
            {
                ["enabled"] = true,
                ["respect_reduced_motion"] = true,
                ["hover_preset"] = "glide_zoom",
                ["launch_preset"] = "glide_launch",
                ["max_scale"] = 1.35,
                ["range_slots"] = 1.7,
                ["hover_ms"] = 180,
                ["elasticity"] = 0.75
            },
            ["monitor"] = new JsonObject { ["display_id"] = "", ["offset"] = 0.0 },
            ["items"] = new JsonArray(),
            ["next_item_id"] = apps.Length + 1
        };

        var items = (JsonArray)root["items"]!;
        for (var index = 0; index < apps.Length; index++)
        {
            var app = apps[index];
            items.Add(new JsonObject
            {
                ["id"] = index + 1,
                ["kind"] = "application",
                ["launch_type"] = "exe",
                ["display_name"] = app.Name,
                ["target_path"] = app.Path
            });
        }

        return root.ToJsonString(new System.Text.Json.JsonSerializerOptions { WriteIndented = true });
    }

    private static void WriteInstallRegistration(
        InstallOptions options,
        string installDirectory,
        bool machineScope)
    {
        var hive = machineScope ? RegistryHive.LocalMachine : RegistryHive.CurrentUser;
        using var root = RegistryKey.OpenBaseKey(hive, RegistryView.Registry64);
        using var uninstall = root.CreateSubKey(UninstallKeyPath, writable: true)
            ?? throw new IOException("无法写入 Glide-Dock 卸载注册信息。");
        var executable = Path.Combine(installDirectory, "glide-dock.exe");
        var uninstaller = Path.Combine(installDirectory, SetupExecutable);
        uninstall.SetValue("DisplayName", ProductName, RegistryValueKind.String);
        uninstall.SetValue("DisplayVersion", ProductVersion, RegistryValueKind.String);
        uninstall.SetValue("Publisher", "Glide-Dock contributors", RegistryValueKind.String);
        uninstall.SetValue("InstallLocation", installDirectory, RegistryValueKind.String);
        uninstall.SetValue("DisplayIcon", executable, RegistryValueKind.String);
        uninstall.SetValue("UninstallString", $"\"{uninstaller}\" --uninstall", RegistryValueKind.String);
        uninstall.SetValue("QuietUninstallString", $"\"{uninstaller}\" --uninstall", RegistryValueKind.String);
        uninstall.SetValue("OwnerSid", options.OwnerSid, RegistryValueKind.String);
        uninstall.SetValue("InstallScope", options.Scope.ToString(), RegistryValueKind.String);
        uninstall.SetValue("NoModify", 1, RegistryValueKind.DWord);
        uninstall.SetValue("NoRepair", 1, RegistryValueKind.DWord);
    }

    private static Dictionary<string, (object Value, RegistryValueKind Kind)>? ReadRegistrationValues(RegistryHive hive)
    {
        try
        {
            using var root = RegistryKey.OpenBaseKey(hive, RegistryView.Registry64);
            using var key = root.OpenSubKey(UninstallKeyPath);
            if (key is null) return null;
            return key.GetValueNames().ToDictionary(
                name => name,
                name => (key.GetValue(name, null, RegistryValueOptions.DoNotExpandEnvironmentNames)!, key.GetValueKind(name)),
                StringComparer.OrdinalIgnoreCase);
        }
        catch
        {
            return null;
        }
    }

    private static void RestoreRegistration(
        RegistryHive hive,
        Dictionary<string, (object Value, RegistryValueKind Kind)>? snapshot)
    {
        try
        {
            using var root = RegistryKey.OpenBaseKey(hive, RegistryView.Registry64);
            root.DeleteSubKeyTree(UninstallKeyPath, throwOnMissingSubKey: false);
            if (snapshot is null) return;
            using var key = root.CreateSubKey(UninstallKeyPath, writable: true);
            if (key is null) return;
            foreach (var (name, item) in snapshot)
            {
                key.SetValue(name, item.Value, item.Kind);
            }
        }
        catch
        {
            // Preserve the original install files even if registry rollback fails.
        }
    }

    private static void CreateShortcuts(InstallOptions options, string installDirectory, bool machineScope)
    {
        var programs = Environment.GetFolderPath(
            machineScope ? Environment.SpecialFolder.CommonPrograms : Environment.SpecialFolder.Programs);
        var group = Path.Combine(programs, ProductName);
        Directory.CreateDirectory(group);
        CreateShortcut(Path.Combine(group, ProductName + ".lnk"),
            Path.Combine(installDirectory, "glide-dock.exe"), installDirectory);
        TryDeleteFile(Path.Combine(group, ProductName + " 设置.lnk"));

        if (options.CreateDesktopShortcut && !options.IsUpdate)
        {
            var desktop = Environment.GetFolderPath(
                machineScope
                    ? Environment.SpecialFolder.CommonDesktopDirectory
                    : Environment.SpecialFolder.DesktopDirectory);
            CreateShortcut(Path.Combine(desktop, ProductName + ".lnk"),
                Path.Combine(installDirectory, "glide-dock.exe"), installDirectory);
        }
    }

    private static void CreateShortcut(string shortcutPath, string targetPath, string workingDirectory)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(shortcutPath)!);
        var shellType = Type.GetTypeFromProgID("WScript.Shell")
            ?? throw new InvalidOperationException("Windows 快捷方式组件不可用。");
        object? shell = null;
        object? shortcut = null;
        try
        {
            shell = Activator.CreateInstance(shellType)
                ?? throw new InvalidOperationException("无法创建 Windows 快捷方式。");
            dynamic shellObject = shell;
            shortcut = shellObject.CreateShortcut(shortcutPath);
            dynamic link = shortcut;
            link.TargetPath = targetPath;
            link.WorkingDirectory = workingDirectory;
            link.IconLocation = targetPath + ",0";
            link.Save();
        }
        finally
        {
            if (shortcut is not null && Marshal.IsComObject(shortcut))
            {
                Marshal.FinalReleaseComObject(shortcut);
            }
            if (shell is not null && Marshal.IsComObject(shell))
            {
                Marshal.FinalReleaseComObject(shell);
            }
        }
    }

    private static void RemoveStartup(string ownerSid, string installDirectory)
    {
        try
        {
            var sid = string.IsNullOrWhiteSpace(ownerSid) ? CurrentOwnerSid() : ownerSid;
            using var users = RegistryKey.OpenBaseKey(RegistryHive.Users, RegistryView.Registry64);
            using var key = users.OpenSubKey(sid + "\\" + RunKeyPath, writable: true);
            if (key?.GetValue(ProductName) is string command
                && command.Contains(installDirectory, StringComparison.OrdinalIgnoreCase))
            {
                key.DeleteValue(ProductName, throwOnMissingValue: false);
            }
        }
        catch
        {
            // Startup cleanup must not block removal of the application files.
        }

        try
        {
            using var process = Process.Start(new ProcessStartInfo
            {
                FileName = "schtasks.exe",
                UseShellExecute = false,
                CreateNoWindow = true,
                ArgumentList = { "/Delete", "/TN", StartupTaskName, "/F" }
            });
            process?.WaitForExit(10000);
        }
        catch
        {
            // Task Scheduler may be unavailable or the task may not exist.
        }
    }

    private static void RemoveShortcuts(InstallScope scope)
    {
        foreach (var programs in new[]
        {
            Environment.GetFolderPath(Environment.SpecialFolder.Programs),
            Environment.GetFolderPath(Environment.SpecialFolder.CommonPrograms)
        }.Distinct(StringComparer.OrdinalIgnoreCase))
        {
            var group = Path.Combine(programs, ProductName);
            TryDeleteFile(Path.Combine(group, ProductName + ".lnk"));
            TryDeleteFile(Path.Combine(group, ProductName + " 设置.lnk"));
            try
            {
                if (Directory.Exists(group) && !Directory.EnumerateFileSystemEntries(group).Any())
                {
                    Directory.Delete(group);
                }
            }
            catch
            {
                // Keep unrelated files and tolerate inaccessible shortcut folders.
            }
        }

        foreach (var desktop in new[]
        {
            Environment.GetFolderPath(Environment.SpecialFolder.DesktopDirectory),
            Environment.GetFolderPath(Environment.SpecialFolder.CommonDesktopDirectory)
        }.Distinct(StringComparer.OrdinalIgnoreCase))
        {
            TryDeleteFile(Path.Combine(desktop, ProductName + ".lnk"));
        }
    }

    private static void RemoveRegistryRegistration(InstallScope scope)
    {
        var hive = scope == InstallScope.AllUsers ? RegistryHive.LocalMachine : RegistryHive.CurrentUser;
        using var root = RegistryKey.OpenBaseKey(hive, RegistryView.Registry64);
        root.DeleteSubKeyTree(UninstallKeyPath, throwOnMissingSubKey: false);
        root.DeleteSubKeyTree(OldUninstallKeyPath, throwOnMissingSubKey: false);
    }

    private static void RemoveLegacyRegistration()
    {
        foreach (var hive in new[] { RegistryHive.LocalMachine, RegistryHive.CurrentUser })
        {
            try
            {
                using var root = RegistryKey.OpenBaseKey(hive, RegistryView.Registry64);
                root.DeleteSubKeyTree(OldUninstallKeyPath, throwOnMissingSubKey: false);
            }
            catch
            {
                // A legacy entry in another scope is not required for a successful install.
            }
        }
    }

    private static UninstallRecord? ReadInstallRecord(RegistryHive hive, InstallScope scope)
    {
        try
        {
            using var root = RegistryKey.OpenBaseKey(hive, RegistryView.Registry64);
            using var key = root.OpenSubKey(UninstallKeyPath);
            var location = key?.GetValue("InstallLocation") as string;
            if (string.IsNullOrWhiteSpace(location))
            {
                return null;
            }
            return new UninstallRecord(
                scope,
                location,
                key?.GetValue("DisplayVersion") as string ?? "未知版本",
                key?.GetValue("OwnerSid") as string ?? string.Empty);
        }
        catch
        {
            return null;
        }
    }

    private static UninstallRecord? ReadLegacyRecord(RegistryHive hive, InstallScope scope)
    {
        try
        {
            using var root = RegistryKey.OpenBaseKey(hive, RegistryView.Registry64);
            using var key = root.OpenSubKey(OldUninstallKeyPath);
            var location = key?.GetValue("InstallLocation") as string;
            if (string.IsNullOrWhiteSpace(location))
            {
                var uninstall = key?.GetValue("UninstallString") as string;
                if (!string.IsNullOrWhiteSpace(uninstall))
                {
                    var exe = ExtractExecutable(uninstall);
                    location = Path.GetDirectoryName(exe);
                }
            }
            if (string.IsNullOrWhiteSpace(location))
            {
                return null;
            }
            return new UninstallRecord(
                scope,
                Path.GetFullPath(location),
                key?.GetValue("DisplayVersion") as string ?? "0.4.1",
                CurrentOwnerSid(),
                IsLegacy: true);
        }
        catch
        {
            return null;
        }
    }

    private static string ExtractExecutable(string command)
    {
        var trimmed = command.Trim();
        if (trimmed.StartsWith('"'))
        {
            var end = trimmed.IndexOf('"', 1);
            return end > 1 ? trimmed[1..end] : trimmed.Trim('"');
        }
        var separator = trimmed.IndexOf(' ');
        return separator < 0 ? trimmed : trimmed[..separator];
    }

    private static void StopProductProcesses()
    {
        KillByImageName("GlideDock.Settings");
        KillByImageName("glide-dock");
    }

    private static void KillByImageName(string imageName)
    {
        try
        {
            using var process = Process.Start(new ProcessStartInfo
            {
                FileName = "taskkill.exe",
                UseShellExecute = false,
                CreateNoWindow = true,
                ArgumentList = { "/F", "/T", "/IM", imageName + ".exe" }
            });
            process?.WaitForExit(10000);
        }
        catch
        {
            foreach (var running in Process.GetProcessesByName(imageName))
            {
                try
                {
                    running.Kill(entireProcessTree: true);
                    running.WaitForExit(5000);
                }
                catch
                {
                    // The next file operation reports a precise sharing violation.
                }
                finally
                {
                    running.Dispose();
                }
            }
        }
    }

    private static void StartDock(string installDirectory)
    {
        var executable = Path.Combine(installDirectory, "glide-dock.exe");
        if (!File.Exists(executable))
        {
            return;
        }
        Process.Start(new ProcessStartInfo
        {
            FileName = executable,
            WorkingDirectory = installDirectory,
            UseShellExecute = true
        });
    }

    public static void LaunchInstalledDock(string installDirectory) => StartDock(installDirectory);

    private static void TryRemoveStartupTask()
    {
        try
        {
            using var process = Process.Start(new ProcessStartInfo
            {
                FileName = "schtasks.exe",
                UseShellExecute = false,
                CreateNoWindow = true,
                ArgumentList = { "/Delete", "/TN", StartupTaskName, "/F" }
            });
            process?.WaitForExit(10000);
        }
        catch
        {
            // A stale task must not block setup.
        }
    }

    private static void WaitForParent(int parentProcessId)
    {
        if (parentProcessId <= 0 || parentProcessId == Environment.ProcessId)
        {
            return;
        }
        try
        {
            using var parent = Process.GetProcessById(parentProcessId);
            parent.WaitForExit(30000);
        }
        catch (ArgumentException)
        {
            // Parent already exited.
        }
    }

    private static void DeleteInstallDirectory(string directory)
    {
        if (!Directory.Exists(directory))
        {
            return;
        }
        var fullPath = Path.GetFullPath(directory).TrimEnd(Path.DirectorySeparatorChar);
        if (string.IsNullOrWhiteSpace(fullPath) || fullPath.Length < 4)
        {
            throw new InvalidOperationException("拒绝删除无效的安装目录。");
        }
        Directory.Delete(fullPath, recursive: true);
    }

    private static void TryDeleteFile(string path)
    {
        try
        {
            if (File.Exists(path))
            {
                File.Delete(path);
            }
        }
        catch
        {
            // Shortcut cleanup is best effort.
        }
    }

    private static string ConfigPath => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), ProductName, "config.json");

    private static string ProductVersion =>
        Assembly.GetExecutingAssembly().GetName().Version?.ToString(3) ?? "0.4.1";
}
