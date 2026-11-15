using System;
using System.IO;

namespace GlideDock.Settings;

internal enum StartupMode
{
    Task,
    Run
}

internal sealed record StartupState(
    StartupTaskInfo Task,
    bool RunExists,
    string? RunCommand);

/// <summary>
/// Coordinates the two autostart mechanisms and remembers the preferred mode.
/// </summary>
internal sealed class StartupManager
{
    private readonly string _modePath;

    public StartupManager(string configPath)
    {
        var directory = Path.GetDirectoryName(Path.GetFullPath(configPath));
        directory = string.IsNullOrWhiteSpace(directory)
            ? AppContext.BaseDirectory
            : directory;
        _modePath = Path.Combine(directory, "startup-mode.txt");
        PreferredMode = ReadPreferredMode();
    }

    public StartupMode PreferredMode { get; private set; }

    public StartupState Query()
    {
        var runExists = StartupRun.TryRead(out var runCommand);
        return new StartupState(StartupTask.Query(), runExists, runCommand);
    }

    public string? Enable(StartupMode mode, string executablePath)
    {
        string? warning = null;
        switch (mode)
        {
            case StartupMode.Task:
                StartupTask.Enable(executablePath);
                try
                {
                    StartupRun.Disable();
                }
                catch (Exception error)
                {
                    warning = "计划任务已启用，但清理兼容 Run 项失败：" + error.Message;
                }
                break;
            case StartupMode.Run:
                StartupRun.Enable(executablePath);
                try
                {
                    StartupTask.Disable();
                }
                catch (StartupCanceledException error)
                {
                    warning = "兼容模式已启用，但计划任务仍然存在：" + error.Message;
                }
                catch (Exception error)
                {
                    warning = "兼容模式已启用，但清理计划任务失败：" + error.Message;
                }
                break;
            default:
                throw new ArgumentOutOfRangeException(nameof(mode));
        }

        SetPreferredMode(mode);
        return warning;
    }

    public void DisableAll()
    {
        var state = Query();
        if (state.Task.Exists)
        {
            // Abort before touching Run if the task cannot be removed. That
            // keeps the UI honest instead of leaving a hidden startup item.
            StartupTask.Disable();
        }
        StartupRun.Disable();
    }

    public void SetPreferredMode(StartupMode mode)
    {
        PreferredMode = mode;
        try
        {
            var directory = Path.GetDirectoryName(_modePath);
            if (!string.IsNullOrWhiteSpace(directory))
            {
                Directory.CreateDirectory(directory);
            }
            File.WriteAllText(_modePath, mode == StartupMode.Task ? "task\n" : "run\n");
        }
        catch
        {
            // The preference is a convenience; never block autostart on it.
        }
    }

    public static string? RegisteredExecutable(StartupState state)
    {
        if (state.Task.Exists && !string.IsNullOrWhiteSpace(state.Task.Command))
        {
            return state.Task.Command.Trim();
        }
        return state.RunExists && !string.IsNullOrWhiteSpace(state.RunCommand)
            ? StartupRun.RegisteredExecutable(state.RunCommand)
            : null;
    }

    private StartupMode ReadPreferredMode()
    {
        try
        {
            if (File.Exists(_modePath)
                && File.ReadAllText(_modePath).Trim().Equals(
                    "run", StringComparison.OrdinalIgnoreCase))
            {
                return StartupMode.Run;
            }
        }
        catch
        {
            // Fall through to the fast default.
        }
        return StartupMode.Task;
    }
}
