using Microsoft.UI;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Windows.UI;

namespace GlideDock.Setup;

internal sealed partial class SetupWindow
{
    private void RenderPage()
    {
        StepList.Children.Clear();
        PageHost.Children.Clear();
        _appChecks.Clear();
        _progressRing = null;
        _progressBar = null;
        _progressPercentText = null;
        _statusText = null;
        BackButton.Visibility = Visibility.Visible;
        BackButton.IsEnabled = !_busy;
        CancelButton.IsEnabled = !_busy;
        CancelButton.Visibility = Visibility.Visible;
        NextButton.IsEnabled = !_busy;
        NextButton.Visibility = Visibility.Visible;
        FooterText.Text = "Glide-Dock 仅在此设备安装，不会更改系统字体。";

        switch (_page)
        {
            case WizardPage.Preflight:
                RenderSteps("环境检查", "安装选项", "首次设置");
                RenderEnvironmentCheck();
                BackButton.Visibility = Visibility.Collapsed;
                CancelButton.Visibility = Visibility.Collapsed;
                NextButton.Visibility = Visibility.Collapsed;
                break;
            case WizardPage.Welcome:
                RenderSteps("欢迎", "安装选项", "首次设置");
                RenderWelcome();
                BackButton.Visibility = Visibility.Collapsed;
                NextButton.Content = _existing is null ? "开始安装" : "原位升级";
                break;
            case WizardPage.Install:
                RenderSteps("欢迎", "安装选项", "首次设置");
                RenderInstallOptions();
                NextButton.Content = "继续";
                break;
            case WizardPage.FirstRun:
                RenderSteps("欢迎", "安装选项", "首次设置");
                RenderFirstRunSetup();
                NextButton.Content = "安装 Glide-Dock";
                break;
            case WizardPage.Progress:
                RenderSteps("欢迎", "安装选项", "安装中");
                RenderProgress("正在安装 Glide-Dock", "正在验证安装位置...", determinate: true);
                BackButton.Visibility = Visibility.Collapsed;
                CancelButton.Visibility = Visibility.Collapsed;
                NextButton.Visibility = Visibility.Collapsed;
                break;
            case WizardPage.Uninstall:
                RenderSteps("卸载确认", "移除程序", "完成");
                RenderUninstallConfirmation();
                BackButton.Visibility = Visibility.Collapsed;
                NextButton.Content = "卸载 Glide-Dock";
                NextButton.IsEnabled = _request.Mode != LaunchMode.UninstallWorker;
                break;
            case WizardPage.UninstallProgress:
                RenderSteps("卸载确认", "移除程序", "完成");
                RenderProgress("正在卸载 Glide-Dock", "正在关闭程序...");
                BackButton.Visibility = Visibility.Collapsed;
                CancelButton.Visibility = Visibility.Collapsed;
                NextButton.Visibility = Visibility.Collapsed;
                break;
            case WizardPage.Finished:
                RenderSteps("欢迎", "安装选项", "完成");
                RenderFinished();
                BackButton.Visibility = Visibility.Collapsed;
                CancelButton.Visibility = Visibility.Collapsed;
                NextButton.Content = "完成";
                break;
        }

        if (_errorText is not null)
        {
            PageHost.Children.Add(_errorText);
        }
    }

    private void RenderSteps(params string[] labels)
    {
        foreach (var label in labels)
        {
            var row = new Grid();
            row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(28) });
            row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            var badge = new Border
            {
                Width = 24,
                Height = 24,
                CornerRadius = new CornerRadius(12),
                Background = Brush(8, 102, 198),
                Child = new TextBlock
                {
                    Text = (StepList.Children.Count + 1).ToString(),
                    Foreground = new SolidColorBrush(Colors.White),
                    FontSize = 12,
                    HorizontalAlignment = HorizontalAlignment.Center,
                    VerticalAlignment = VerticalAlignment.Center
                }
            };
            row.Children.Add(badge);
            var text = new TextBlock
            {
                Text = label,
                FontSize = 13,
                Foreground = Brush(49, 61, 71),
                VerticalAlignment = VerticalAlignment.Center,
                Margin = new Thickness(10, 0, 0, 0)
            };
            Grid.SetColumn(text, 1);
            row.Children.Add(text);
            StepList.Children.Add(row);
        }
    }

    private void RenderWelcome()
    {
        AddEyebrow(_existing is null ? "GLIDE-DOCK SETUP" : "已检测到现有安装");
        AddHeading(_existing is null ? "把常用应用放在指尖" : "升级 Glide-Dock");
        AddBody(_existing is null
            ? "安装你自己的桌面 Dock。安装完成后，你可以选择外观、开机启动和推荐应用。"
            : $"当前安装版本：{_existing.Version}\n安装位置：{_existing.InstallDirectory}\n\n继续将原位升级并保留你的配置和启动偏好。也可以取消，本机现有安装不会更改。",
            2);

        var image = new Image
        {
            Height = 250,
            Stretch = Stretch.Uniform,
            HorizontalAlignment = HorizontalAlignment.Left,
            Margin = new Thickness(0, 8, 0, 4)
        };
        _ = LoadImageAsync("GlideDock.Setup.Brand.Preview.png").ContinueWith(task =>
        {
            if (task.Status == TaskStatus.RanToCompletion && task.Result is not null)
            {
                DispatcherQueue.TryEnqueue(() => image.Source = task.Result);
            }
        });
        PageHost.Children.Add(new Border
        {
            Background = new SolidColorBrush(Colors.White),
            BorderBrush = Brush(226, 231, 235),
            BorderThickness = new Thickness(1),
            CornerRadius = new CornerRadius(8),
            Padding = new Thickness(12),
            HorizontalAlignment = HorizontalAlignment.Left,
            Child = image
        });
        AddNote(_existing is null
            ? "推荐应用仅包含本机检测到的程序，可在首次设置中勾选或全部跳过。"
            : _existing.IsLegacy
                ? "升级会迁移旧安装记录，不会创建并行副本。"
                : "不会创建并行安装。升级期间会暂时关闭 Dock 和设置中心。");

        if (_environmentChecks.Count > 0)
        {
            AddLabel("启动环境检查", 14);
            foreach (var check in _environmentChecks)
            {
                var color = check.State switch
                {
                    CheckState.Passed => Brush(16, 124, 92),
                    CheckState.Warning => Brush(181, 112, 0),
                    _ => Brush(190, 54, 48)
                };
                var row = new Grid { ColumnSpacing = 10, Margin = new Thickness(0, 2, 0, 2) };
                row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(8) });
                row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
                row.Children.Add(new Border
                {
                    Width = 8,
                    Height = 8,
                    CornerRadius = new CornerRadius(4),
                    Background = color,
                    VerticalAlignment = VerticalAlignment.Top,
                    Margin = new Thickness(0, 6, 0, 0)
                });
                var detail = new StackPanel { Spacing = 1 };
                detail.Children.Add(new TextBlock
                {
                    Text = check.Title,
                    FontSize = 13,
                    FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
                    Foreground = Brush(47, 57, 66)
                });
                detail.Children.Add(new TextBlock
                {
                    Text = check.Detail,
                    FontSize = 12,
                    Foreground = Brush(113, 124, 133),
                    TextWrapping = TextWrapping.Wrap
                });
                Grid.SetColumn(detail, 1);
                row.Children.Add(detail);
                PageHost.Children.Add(row);
            }
        }
        NextButton.IsEnabled = _preflightPassed;
    }

    private void RenderEnvironmentCheck()
    {
        AddEyebrow("启动检查");
        AddHeading("正在检查安装环境");
        AddBody("检查 Windows 版本、系统架构、安装包完整性和默认位置空间。", 2);
        _progressRing = new ProgressRing
        {
            IsActive = true,
            Width = 34,
            Height = 34,
            Margin = new Thickness(0, 14, 0, 6)
        };
        PageHost.Children.Add(_progressRing);
        AddNote("检查完成后才会进入安装向导。", 2);
    }

    private void RenderInstallOptions()
    {
        AddEyebrow("安装选项");
        AddHeading("选择安装位置");
        AddBody("安装目录只存放程序文件。你的 Dock 配置和偏好仍保存在当前 Windows 用户的个人资料中。");
        AddLabel("安装范围");
        _scopeBox = new ComboBox { MinWidth = 450, HorizontalAlignment = HorizontalAlignment.Left };
        _scopeBox.Items.Add(new ComboBoxItem { Content = "所有用户", Tag = InstallScope.AllUsers });
        _scopeBox.Items.Add(new ComboBoxItem { Content = "仅当前用户", Tag = InstallScope.CurrentUser });
        _scopeBox.SelectedIndex = _options.Scope == InstallScope.AllUsers ? 0 : 1;
        _scopeBox.SelectionChanged += Scope_SelectionChanged;
        PageHost.Children.Add(_scopeBox);
        AddNote("所有用户安装需要管理员授权；仅当前用户安装不需要管理员权限。");

        AddLabel("安装目录");
        var pathRow = new Grid();
        pathRow.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        pathRow.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        _installPathBox = new TextBox
        {
            Text = _options.InstallDirectory,
            MinWidth = 420,
            PlaceholderText = "选择 Glide-Dock 安装目录"
        };
        pathRow.Children.Add(_installPathBox);
        var browse = new Button { Content = "浏览...", Margin = new Thickness(10, 0, 0, 0), MinWidth = 90 };
        browse.Click += Browse_Click;
        Grid.SetColumn(browse, 1);
        pathRow.Children.Add(browse);
        PageHost.Children.Add(pathRow);

        _desktopCheck = new CheckBox
        {
            Content = "在桌面创建快捷方式",
            IsChecked = _options.CreateDesktopShortcut,
            Margin = new Thickness(0, 12, 0, 0)
        };
        PageHost.Children.Add(_desktopCheck);
        AddNote("开始菜单快捷方式会自动创建。桌面快捷方式默认关闭。");
        if (_hasExistingConfig)
        {
            AddNote("检测到已有用户配置，首次设置将保留该配置，不会重置 Dock 内容。");
        }
    }

    private void RenderFirstRunSetup()
    {
        AddEyebrow("首次运行");
        AddHeading(_hasExistingConfig ? "保留你的 Dock 配置" : "设置你的 Dock");
        AddBody(_hasExistingConfig
            ? "检测到此 Windows 用户已有 Glide-Dock 配置。为避免覆盖，外观和 Dock 条目将保持不变。"
            : "这些选项仅在没有现有配置时写入。所有选项都可以稍后在设置中心调整。");
        if (_hasExistingConfig)
        {
            AddNote("可在设置中心继续调整主题、启动行为和 Dock 应用。点击安装以保留现有设置。", 16);
            return;
        }

        AddLabel("主题");
        _themeChoices = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 18 };
        _themeChoices.Children.Add(ThemeRadio("跟随系统", "system"));
        _themeChoices.Children.Add(ThemeRadio("浅色", "light"));
        _themeChoices.Children.Add(ThemeRadio("深色", "dark"));
        PageHost.Children.Add(_themeChoices);
        _startupCheck = new CheckBox
        {
            Content = "登录 Windows 后自动启动 Glide-Dock",
            IsChecked = _options.EnableStartup,
            Margin = new Thickness(0, 8, 0, 0)
        };
        PageHost.Children.Add(_startupCheck);

        AddLabel("推荐应用", 10);
        var appPanel = new StackPanel { Spacing = 2 };
        foreach (var app in _availableApps)
        {
            var check = new CheckBox
            {
                Content = app.Name,
                IsChecked = _options.SelectedApps.Contains(app.Id),
                Tag = app.Id
            };
            _appChecks[app.Id] = check;
            appPanel.Children.Add(check);
        }
        PageHost.Children.Add(new Border
        {
            Background = new SolidColorBrush(Colors.White),
            BorderBrush = Brush(226, 231, 235),
            BorderThickness = new Thickness(1),
            CornerRadius = new CornerRadius(8),
            Padding = new Thickness(14, 10, 14, 10),
            Child = appPanel
        });
        AddNote("只显示本机已检测到的应用；取消全部即可从空 Dock 开始。", 4);
    }

    private void RenderProgress(string title, string message, bool determinate = false)
    {
        AddEyebrow("GLIDE-DOCK");
        AddHeading(title);
        if (determinate)
        {
            var progressRow = new Grid
            {
                ColumnSpacing = 14,
                Margin = new Thickness(0, 18, 0, 8),
                VerticalAlignment = VerticalAlignment.Center
            };
            progressRow.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            progressRow.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            _progressBar = new ProgressBar
            {
                Minimum = 0,
                Maximum = 100,
                Value = 0,
                Height = 6,
                VerticalAlignment = VerticalAlignment.Center
            };
            progressRow.Children.Add(_progressBar);
            _progressPercentText = new TextBlock
            {
                Text = "0%",
                FontSize = 13,
                MinWidth = 38,
                TextAlignment = TextAlignment.Right,
                Foreground = Brush(84, 96, 106),
                VerticalAlignment = VerticalAlignment.Center
            };
            Grid.SetColumn(_progressPercentText, 1);
            progressRow.Children.Add(_progressPercentText);
            PageHost.Children.Add(progressRow);
        }
        else
        {
            _progressRing = new ProgressRing
            {
                IsActive = true,
                Width = 34,
                Height = 34,
                Margin = new Thickness(0, 14, 0, 6)
            };
            PageHost.Children.Add(_progressRing);
        }
        _statusText = AddBody(message);
    }

    private void RenderUninstallConfirmation()
    {
        AddEyebrow("GLIDE-DOCK UNINSTALLER");
        AddHeading("卸载 Glide-Dock");
        if (_existing is null)
        {
            AddBody("没有找到已注册的 Glide-Dock 安装。", 2);
            NextButton.IsEnabled = false;
            return;
        }
        AddBody($"版本：{_existing.Version}\n安装位置：{_existing.InstallDirectory}\n安装范围：{(_existing.Scope == InstallScope.AllUsers ? "所有用户" : "仅当前用户")}", 2);
        AddNote("卸载会移除程序文件、快捷方式和 Glide-Dock 启动项，但会保留 %APPDATA%\\Glide-Dock 中的配置与偏好。", 14);
    }

    private void RenderFinished()
    {
        AddEyebrow("完成");
        AddHeading(_completed ? "Glide-Dock 已安装" : "Glide-Dock 已卸载");
        AddBody(_completed ? "安装完成。你的 Dock 配置已准备好。" : "程序文件和快捷方式已移除；用户配置仍保留在本机。", 2);
        if (_completed)
        {
            _launchCheck = new CheckBox
            {
                Content = "完成后启动 Glide-Dock",
                IsChecked = _options.LaunchAfterInstall,
                Margin = new Thickness(0, 18, 0, 0)
            };
            PageHost.Children.Add(_launchCheck);
        }
    }

    private void AddEyebrow(string text) => PageHost.Children.Add(new TextBlock
    {
        Text = text,
        FontSize = 12,
        FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
        Foreground = Brush(8, 102, 198),
        Margin = new Thickness(0, 0, 0, 2)
    });

    private void AddHeading(string text) => PageHost.Children.Add(new TextBlock
    {
        Text = text,
        FontSize = 30,
        FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
        Foreground = Brush(31, 40, 48),
        TextWrapping = TextWrapping.Wrap,
        Margin = new Thickness(0, 0, 0, 2)
    });

    private TextBlock AddBody(string text, int lineSpacing = 1)
    {
        var body = new TextBlock
        {
            Text = text,
            FontSize = 15,
            Foreground = Brush(84, 96, 106),
            TextWrapping = TextWrapping.Wrap,
            LineHeight = 24,
            Margin = new Thickness(0, 2, 0, 4)
        };
        PageHost.Children.Add(body);
        return body;
    }

    private void AddLabel(string text, int top = 16) => PageHost.Children.Add(new TextBlock
    {
        Text = text,
        FontSize = 14,
        FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
        Foreground = Brush(47, 57, 66),
        Margin = new Thickness(0, top, 0, 4)
    });

    private void AddNote(string text, int top = 8) => PageHost.Children.Add(new TextBlock
    {
        Text = text,
        FontSize = 12,
        Foreground = Brush(113, 124, 133),
        TextWrapping = TextWrapping.Wrap,
        LineHeight = 19,
        Margin = new Thickness(0, top, 0, 0)
    });

    private static SolidColorBrush Brush(byte r, byte g, byte b) =>
        new(Color.FromArgb(255, r, g, b));

    private RadioButton ThemeRadio(string label, string theme) => new()
    {
        Content = label,
        Tag = theme,
        GroupName = "appearance-theme",
        IsChecked = string.Equals(_options.Theme, theme, StringComparison.Ordinal)
    };
}
