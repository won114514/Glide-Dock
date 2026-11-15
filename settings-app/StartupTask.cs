using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Security.Principal;
using System.Text;
using System.Xml.Linq;

namespace GlideDock.Settings;

internal sealed record StartupTaskInfo(
    bool Exists,
    bool Enabled,
    string? Command,
    string? Arguments);

internal sealed class StartupCanceledException(string message) : Exception(message);

/// <summary>
/// Registers the fast path: a current-user logon task with no scheduler delay.
/// </summary>
internal static class StartupTask
{
    private const string TaskName = "Glide-Dock";
    private static readonly XNamespace Ns = "http://schemas.microsoft.com/windows/2004/02/mit/task";

    public static StartupTaskInfo Query()
    {
        var result = RunSchtasks(
            ["/Query", "/TN", TaskName, "/XML"],
            elevated: false);
        if (result.ExitCode != 0 || string.IsNullOrWhiteSpace(result.StandardOutput))
        {
            return new StartupTaskInfo(false, false, null, null);
        }

        var document = XDocument.Parse(result.StandardOutput);
        var settings = document.Descendants()
            .FirstOrDefault(element => element.Name.LocalName == "Settings");
        var trigger = document.Descendants()
            .FirstOrDefault(element => element.Name.LocalName == "LogonTrigger");
        var exec = document.Descendants()
            .FirstOrDefault(element => element.Name.LocalName == "Exec");
        var command = exec?.Elements()
            .FirstOrDefault(element => element.Name.LocalName == "Command")?.Value;
        var arguments = exec?.Elements()
            .FirstOrDefault(element => element.Name.LocalName == "Arguments")?.Value;
        var settingsEnabled = BoolValue(settings, "Enabled", true);
        var triggerEnabled = BoolValue(trigger, "Enabled", true);
        return new StartupTaskInfo(
            Exists: true,
            Enabled: settingsEnabled && triggerEnabled,
            Command: command,
            Arguments: arguments);
    }

    public static void Enable(string executablePath)
    {
        var path = StartupRun.ValidateExecutable(executablePath);
        var xmlPath = Path.Combine(
            Path.GetTempPath(),
            $"glide-dock-task-{Guid.NewGuid():N}.xml");
        try
        {
            File.WriteAllText(xmlPath, BuildXml(path), Encoding.Unicode);
            var result = RunSchtasks(
                ["/Create", "/TN", TaskName, "/XML", xmlPath, "/F"],
                elevated: true);
            if (result.ExitCode != 0)
            {
                throw new InvalidOperationException(
                    $"注册计划任务失败（schtasks 退出码 {result.ExitCode}）。");
            }
        }
        finally
        {
            try
            {
                File.Delete(xmlPath);
            }
            catch
            {
                // The temp XML is harmless if a scanner still holds it briefly.
            }
        }
    }

    public static void Disable()
    {
        if (!Query().Exists)
        {
            return;
        }

        var normal = RunSchtasks(["/Delete", "/TN", TaskName, "/F"], elevated: false);
        if (normal.ExitCode == 0)
        {
            return;
        }

        var elevated = RunSchtasks(["/Delete", "/TN", TaskName, "/F"], elevated: true);
        if (elevated.ExitCode != 0)
        {
            throw new InvalidOperationException(
                $"删除计划任务失败（schtasks 退出码 {elevated.ExitCode}）。");
        }
    }

    private static string BuildXml(string executablePath)
    {
        var user = CurrentUser();
        var workingDirectory = Path.GetDirectoryName(executablePath) ?? string.Empty;
        var document = new XDocument(
            new XElement(Ns + "Task",
                new XAttribute("version", "1.2"),
                new XElement(Ns + "RegistrationInfo",
                    new XElement(Ns + "Description", "Glide-Dock login startup"),
                    new XElement(Ns + "Author", "Glide-Dock")),
                new XElement(Ns + "Triggers",
                    new XElement(Ns + "LogonTrigger",
                        new XElement(Ns + "Enabled", "true"),
                        new XElement(Ns + "UserId", user),
                        new XElement(Ns + "Delay", "PT0S"))),
                new XElement(Ns + "Principals",
                    new XElement(Ns + "Principal",
                        new XAttribute("id", "Author"),
                        new XElement(Ns + "UserId", user),
                        new XElement(Ns + "LogonType", "InteractiveToken"),
                        new XElement(Ns + "RunLevel", "LeastPrivilege"))),
                new XElement(Ns + "Settings",
                    new XElement(Ns + "MultipleInstancesPolicy", "IgnoreNew"),
                    new XElement(Ns + "DisallowStartIfOnBatteries", "false"),
                    new XElement(Ns + "StopIfGoingOnBatteries", "false"),
                    new XElement(Ns + "AllowHardTerminate", "true"),
                    new XElement(Ns + "StartWhenAvailable", "true"),
                    new XElement(Ns + "RunOnlyIfNetworkAvailable", "false"),
                    new XElement(Ns + "IdleSettings",
                        new XElement(Ns + "StopOnIdleEnd", "false"),
                        new XElement(Ns + "RestartOnIdle", "false")),
                    new XElement(Ns + "AllowStartOnDemand", "true"),
                    new XElement(Ns + "Enabled", "true"),
                    new XElement(Ns + "Hidden", "false"),
                    new XElement(Ns + "RunOnlyIfIdle", "false"),
                    new XElement(Ns + "WakeToRun", "false"),
                    new XElement(Ns + "ExecutionTimeLimit", "PT0S"),
                    new XElement(Ns + "Priority", "4")),
                new XElement(Ns + "Actions",
                    new XAttribute("Context", "Author"),
                    new XElement(Ns + "Exec",
                        new XElement(Ns + "Command", executablePath),
                        new XElement(Ns + "Arguments", "--autostart"),
                        new XElement(Ns + "WorkingDirectory", workingDirectory)))));
        return "<?xml version=\"1.0\" encoding=\"utf-16\"?>\r\n"
            + document.Root!.ToString(SaveOptions.DisableFormatting);
    }

    private static bool BoolValue(XElement? parent, string name, bool fallback)
    {
        var value = parent?.Elements()
            .FirstOrDefault(element => element.Name.LocalName == name)?.Value;
        return bool.TryParse(value, out var parsed) ? parsed : fallback;
    }

    private static string CurrentUser()
    {
        try
        {
            return WindowsIdentity.GetCurrent().Name;
        }
        catch
        {
            return $"{Environment.UserDomainName}\\{Environment.UserName}";
        }
    }

    private static (int ExitCode, string StandardOutput, string StandardError) RunSchtasks(
        IReadOnlyList<string> arguments,
        bool elevated)
    {
        var executable = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.System),
            "schtasks.exe");
        var startInfo = new ProcessStartInfo
        {
            FileName = executable,
            UseShellExecute = elevated,
            CreateNoWindow = !elevated,
            WindowStyle = ProcessWindowStyle.Hidden
        };
        if (elevated)
        {
            startInfo.Verb = "runas";
            startInfo.Arguments = string.Join(" ", arguments.Select(QuoteArgument));
        }
        else
        {
            startInfo.RedirectStandardOutput = true;
            startInfo.RedirectStandardError = true;
            foreach (var argument in arguments)
            {
                startInfo.ArgumentList.Add(argument);
            }
        }

        Process? process;
        try
        {
            process = Process.Start(startInfo);
        }
        catch (Win32Exception error) when (error.NativeErrorCode == 1223)
        {
            throw new StartupCanceledException("已取消管理员授权。");
        }

        if (process is null)
        {
            throw new InvalidOperationException("无法启动 schtasks.exe。");
        }

        using (process)
        {
            if (!process.WaitForExit(30_000))
            {
                try
                {
                    process.Kill(entireProcessTree: true);
                }
                catch
                {
                    // Best effort cleanup after a hung elevated helper.
                }
                throw new TimeoutException("等待计划任务操作超时。");
            }

            var stdout = elevated ? string.Empty : process.StandardOutput.ReadToEnd();
            var stderr = elevated ? string.Empty : process.StandardError.ReadToEnd();
            return (process.ExitCode, stdout, stderr);
        }
    }

    private static string QuoteArgument(string value)
    {
        if (value.Length == 0)
        {
            return "\"\"";
        }
        if (!value.Any(char.IsWhiteSpace) && !value.Contains('"'))
        {
            return value;
        }

        var builder = new StringBuilder("\"");
        var backslashes = 0;
        foreach (var character in value)
        {
            if (character == '\\')
            {
                backslashes++;
                continue;
            }
            if (character == '"')
            {
                builder.Append('\\', backslashes * 2 + 1);
                builder.Append('"');
                backslashes = 0;
                continue;
            }
            builder.Append('\\', backslashes);
            backslashes = 0;
            builder.Append(character);
        }
        builder.Append('\\', backslashes * 2);
        builder.Append('"');
        return builder.ToString();
    }
}
