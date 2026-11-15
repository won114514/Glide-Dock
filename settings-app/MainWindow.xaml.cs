#nullable disable
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Reflection;
using GlideDock.Settings.Controls;
using Microsoft.UI;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Text;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Animation;
using Microsoft.Win32;
using WinRT.Interop;
using Windows.ApplicationModel.DataTransfer;
using Windows.Graphics;
using Windows.UI;

namespace GlideDock.Settings;

public sealed partial class MainWindow : Window
{
	private sealed record SearchEntry(string Title, string Description, string PageTag, string Keywords);

	private static readonly Dictionary<string, (string Title, string Subtitle)> PageInfo = new Dictionary<string, (string, string)>
	{
		["general"] = ("常规", "主题、强调色与配置文件。"),
		["appearance"] = ("外观", "尺寸、材质与位置，改动会立即生效。"),
		["behavior"] = ("行为", "悬停、提示与运行状态。"),
		["animation"] = ("动画", "选择悬停与启动动效，调校速度与弹性。"),
		["diagnostics"] = ("诊断与维护", "检查权限、拖放、守护进程和启动状态。"),
		["about"] = ("关于", "版本、许可证与配置文件。")
	};

	private readonly SettingsDocument _doc;

	private readonly StartupManager _startup;

	private readonly nint _owner;

	private DispatcherQueueTimer _saveTimer;

	private DispatcherQueueTimer _ownerTimer;

	private DispatcherQueueTimer _placementTimer;

	private readonly List<SearchEntry> _searchIndex = new List<SearchEntry>();

	private AppWindow _appWindow;

	private bool _loading = true;

	private bool _startupBusy;

	private bool _startupSyncing;
	private DateTime _lastKnownWriteUtc;
	private bool _initialPageApplied;
	private bool _saving;
	private FileSystemWatcher _configWatcher;
	private DispatcherQueueTimer _syncTimer;

	private bool _dark;

	private bool _micaAvailable;

	private string _currentPage = "general";
	private DiagnosticsSnapshot _lastDiagnostics;

	private PointInt32? _lastPlacement;

	private static readonly (string Id, string Name)[] AccentColors = new(string, string)[5]
	{
		("blue", "蓝色"),
		("teal", "青绿"),
		("violet", "紫罗兰"),
		("orange", "暖橙"),
		("pink", "玫瑰")
	};


	private string PlacementPath
	{
		get
		{
			string text = Path.GetDirectoryName(Path.GetFullPath(_doc.Path));
			if (string.IsNullOrWhiteSpace(text))
			{
				text = AppContext.BaseDirectory;
			}
			return Path.Combine(text, "settings-window.txt");
		}
	}

	private bool AttachedToDock => _owner != IntPtr.Zero;

	public MainWindow(string configPath, nint owner)
	{
		_owner = owner;
		_doc = SettingsDocument.Load(configPath);
		_startup = new StartupManager(_doc.Path);
		InitializeComponent();
		ConfigureSliderRanges();
		base.ExtendsContentIntoTitleBar = true;
		SetTitleBar(TitleBarDragRegion);
		ConfigureWindow();
		ConfigureBackdrop();
		BuildSearchIndex();
		CreateTimers();
		LoadFromDocument();
		StartConfigWatcher();
		ApplyTheme();
		SelectPage("general");
		_loading = false;
		RootGrid.Loaded += delegate
		{
			if (!_initialPageApplied)
			{
				_initialPageApplied = true;
				SelectPage("general");
			}
		};
		base.Activated += MainWindow_Activated;
	}

	private void ConfigureSliderRanges()
	{
		SetRange(IconSizeSlider, 24.0, 128.0, 1.0);
		SetRange(SpacingSlider, 0.0, 48.0, 1.0);
		SetRange(PaddingSlider, 4.0, 48.0, 1.0);
		SetRange(CornerRadiusSlider, 0.0, 64.0, 1.0);
		SetRange(OpacitySlider, 20.0, 100.0, 1.0);
		SetRange(EdgeMarginSlider, 0.0, 200.0, 1.0);
		SetRange(OffsetSlider, -500.0, 500.0, 1.0);
		SetRange(MaxScaleSlider, 1.05, 3.5, 0.05);
		SetRange(RangeSlider, 0.5, 8.0, 0.1);
		SetRange(HoverMsSlider, 80.0, 400.0, 5.0);
		SetRange(ElasticitySlider, 0.0, 100.0, 1.0);
	}

	private static void SetRange(Slider slider, double minimum, double maximum, double step)
	{
		slider.Minimum = minimum;
		slider.Maximum = maximum;
		slider.StepFrequency = step;
	}

	private void ConfigureWindow()
	{
		nint windowHandle = WindowNative.GetWindowHandle(this);
		WindowId windowIdFromWindow = Win32Interop.GetWindowIdFromWindow(windowHandle);
		_appWindow = AppWindow.GetFromWindowId(windowIdFromWindow);
		var iconPath = Path.Combine(AppContext.BaseDirectory, "Assets", "Glide-Dock-FullColor.ico");
		if (File.Exists(iconPath))
		{
			_appWindow.SetIcon(iconPath);
		}
		_appWindow.Title = "Glide-Dock 设置";
		if (_appWindow.Presenter is OverlappedPresenter overlappedPresenter)
		{
			overlappedPresenter.IsResizable = false;
			overlappedPresenter.IsMaximizable = false;
			overlappedPresenter.IsMinimizable = true;
		}
		RectInt32 workArea = DisplayArea.GetFromWindowId(windowIdFromWindow, DisplayAreaFallback.Primary).WorkArea;
		uint dpiForWindow = NativeMethods.GetDpiForWindow(windowHandle);
		double num = Math.Max(1.0, (double)dpiForWindow / 96.0);
		int num2 = Math.Min((int)Math.Round(1180.0 * num), Math.Max(820, workArea.Width - 80));
		int num3 = Math.Min((int)Math.Round(900.0 * num), Math.Max(620, workArea.Height - 80));
		_appWindow.Resize(new SizeInt32(num2, num3));
		PointInt32 defaultValue = new PointInt32(workArea.X + (workArea.Width - num2) / 2, workArea.Y + (workArea.Height - num3) / 2);
		PointInt32 valueOrDefault = LoadPlacement().GetValueOrDefault(defaultValue);
		RectInt32 workArea2 = DisplayArea.GetFromPoint(valueOrDefault, DisplayAreaFallback.Nearest).WorkArea;
		int max = Math.Max(workArea2.X, workArea2.X + workArea2.Width - num2);
		int max2 = Math.Max(workArea2.Y, workArea2.Y + workArea2.Height - num3);
		PointInt32 value = new PointInt32(Math.Clamp(valueOrDefault.X, workArea2.X, max), Math.Clamp(valueOrDefault.Y, workArea2.Y, max2));
		NativeMethods.SetWindowPos(windowHandle, IntPtr.Zero, value.X, value.Y, num2, num3, 20u);
		_lastPlacement = value;
		_appWindow.TitleBar.ExtendsContentIntoTitleBar = true;
		_appWindow.TitleBar.ButtonBackgroundColor = Color.FromArgb(byte.MaxValue, 246, 246, 250);
		_appWindow.TitleBar.ButtonInactiveBackgroundColor = Color.FromArgb(byte.MaxValue, 246, 246, 250);
		_appWindow.TitleBar.ButtonHoverBackgroundColor = Color.FromArgb(byte.MaxValue, 232, 232, 238);
		_appWindow.TitleBar.ButtonPressedBackgroundColor = Color.FromArgb(byte.MaxValue, 220, 220, 228);
		RootGrid.ActualThemeChanged += delegate
		{
			_dark = RootGrid.ActualTheme == ElementTheme.Dark;
			UpdateTitleBarColors();
		};
	}

	private void ConfigureBackdrop()
	{
		try
		{
			base.SystemBackdrop = new MicaBackdrop();
			_micaAvailable = true;
		}
		catch
		{
			_micaAvailable = false;
		}
	}

	private void CreateTimers()
	{
		DispatcherQueue forCurrentThread = DispatcherQueue.GetForCurrentThread();
		_saveTimer = forCurrentThread.CreateTimer();
		_saveTimer.Interval = TimeSpan.FromMilliseconds(350.0);
		_saveTimer.IsRepeating = false;
		_saveTimer.Tick += delegate
		{
			SaveNow();
		};
		_syncTimer = forCurrentThread.CreateTimer();
		_syncTimer.Interval = TimeSpan.FromMilliseconds(250.0);
		_syncTimer.IsRepeating = false;
		_syncTimer.Tick += delegate
		{
			ReloadIfChanged();
		};
		_ownerTimer = forCurrentThread.CreateTimer();
		_ownerTimer.Interval = TimeSpan.FromSeconds(1.0);
		_ownerTimer.IsRepeating = true;
		_ownerTimer.Tick += delegate
		{
			if (_owner != IntPtr.Zero && !NativeMethods.IsWindow(_owner))
			{
				Close();
			}
		};
		_ownerTimer.Start();
		_placementTimer = forCurrentThread.CreateTimer();
		_placementTimer.Interval = TimeSpan.FromSeconds(1.0);
		_placementTimer.IsRepeating = true;
		_placementTimer.Tick += delegate
		{
			SavePlacement();
		};
		_placementTimer.Start();
		_appWindow.Changed += delegate(AppWindow _, AppWindowChangedEventArgs args)
		{
			if (args.DidPositionChange)
			{
				SavePlacement();
			}
		};
		base.Closed += delegate
		{
			_saveTimer.Stop();
			_ownerTimer.Stop();
			_placementTimer.Stop();
			_syncTimer?.Stop();
			_configWatcher?.Dispose();
			SavePlacement();
			SaveNow();
		};
	}

	private PointInt32? LoadPlacement()
	{
		try
		{
			string placementPath = PlacementPath;
			if (!File.Exists(placementPath))
			{
				return null;
			}
			string[] array = File.ReadAllLines(placementPath);
			if (array.Length >= 2 && int.TryParse(array[0], out var result) && int.TryParse(array[1], out var result2))
			{
				return new PointInt32(result, result2);
			}
		}
		catch
		{
		}
		return null;
	}

	private void SavePlacement()
	{
		if (!NativeMethods.GetWindowRect(WindowNative.GetWindowHandle(this), out var rect))
		{
			return;
		}
		PointInt32 value = new PointInt32(rect.Left, rect.Top);
		PointInt32? lastPlacement = _lastPlacement;
		if (lastPlacement.HasValue)
		{
			PointInt32 valueOrDefault = lastPlacement.GetValueOrDefault();
			if (valueOrDefault.X == value.X && valueOrDefault.Y == value.Y)
			{
				return;
			}
		}
		_lastPlacement = value;
		try
		{
			string directoryName = Path.GetDirectoryName(PlacementPath);
			if (!string.IsNullOrEmpty(directoryName))
			{
				Directory.CreateDirectory(directoryName);
			}
			File.WriteAllText(PlacementPath, $"{value.X}\n{value.Y}\n");
		}
		catch
		{
		}
	}

	private void LoadFromDocument()
	{
		_loading = true;
		ComboBox themeCombo = ThemeCombo;
		string theme = _doc.Theme;
		int selectedIndex = ((theme == "light") ? 1 : ((theme == "dark") ? 2 : 0));
		themeCombo.SelectedIndex = selectedIndex;
		SelectCombo(LayerCombo, _doc.Layer);
		SetSlider(IconSizeSlider, _doc.IconSize);
		SetSlider(SpacingSlider, _doc.Spacing);
		SetSlider(PaddingSlider, _doc.Padding);
		SetSlider(CornerRadiusSlider, _doc.CornerRadius);
		SetSlider(OpacitySlider, _doc.Opacity * 100f);
		SetSlider(EdgeMarginSlider, _doc.EdgeMargin);
		SetSlider(OffsetSlider, _doc.MonitorOffset);
		MagnificationToggle.IsOn = _doc.Magnification;
		TooltipsToggle.IsOn = _doc.ShowTooltips;
		LaunchAnimationToggle.IsOn = _doc.LaunchAnimation;
		RunningIndicatorToggle.IsOn = _doc.ShowRunningIndicator;
		RefreshStartupUi();
		MotionEnabledToggle.IsOn = _doc.MotionEnabled;
		ReducedMotionToggle.IsOn = _doc.RespectReducedMotion;
		SelectCombo(HoverPresetCombo, _doc.HoverPreset);
		SelectCombo(LaunchPresetCombo, _doc.LaunchPreset);
		UpdateClassicDurationState();
		SetSlider(MaxScaleSlider, _doc.MaxScale);
		SetSlider(HoverMsSlider, _doc.HoverMilliseconds);
		SetSlider(ElasticitySlider, _doc.Elasticity * 100f);
		SetSlider(RangeSlider, _doc.RangeSlots);
		UpdateRecommendation();
		ConfigCard.Description = _doc.Path;
		AboutConfigPath.Text = _doc.Path;
		string text = Assembly.GetExecutingAssembly().GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion ?? "0.1.0";
		VersionText.Text = "v" + text.Split('+')[0];
		_lastKnownWriteUtc = ReadConfigWriteTime();
		_loading = false;
		UpdateMagnificationEnabled();
	}

	private DateTime ReadConfigWriteTime()
	{
		try
		{
			return File.Exists(_doc.Path) ? File.GetLastWriteTimeUtc(_doc.Path) : DateTime.MinValue;
		}
		catch
		{
			return DateTime.MinValue;
		}
	}

	private void StartConfigWatcher()
	{
		try
		{
			string directory = Path.GetDirectoryName(_doc.Path);
			string fileName = Path.GetFileName(_doc.Path);
			if (string.IsNullOrWhiteSpace(directory) || string.IsNullOrWhiteSpace(fileName))
			{
				return;
			}
			_configWatcher = new FileSystemWatcher(directory, fileName)
			{
				NotifyFilter = NotifyFilters.LastWrite | NotifyFilters.Size | NotifyFilters.FileName | NotifyFilters.CreationTime,
				EnableRaisingEvents = true
			};
			FileSystemEventHandler handler = delegate
			{
				DispatcherQueue.TryEnqueue(delegate
				{
					if (_syncTimer is not null)
					{
						_syncTimer.Stop();
						_syncTimer.Start();
					}
				});
			};
			_configWatcher.Changed += handler;
			_configWatcher.Created += handler;
			_configWatcher.Renamed += delegate
			{
				DispatcherQueue.TryEnqueue(delegate
				{
					if (_syncTimer is not null)
					{
						_syncTimer.Stop();
						_syncTimer.Start();
					}
				});
			};
		}
		catch (Exception)
		{
		}
	}

	private void ReloadIfChanged()
	{
		if (_loading || _saving || _saveTimer is { IsRunning: true })
		{
			return;
		}
		DateTime dateTime = ReadConfigWriteTime();
		if (dateTime == _lastKnownWriteUtc)
		{
			return;
		}
		try
		{
			_doc.Reload();
			_lastKnownWriteUtc = dateTime;
			LoadFromDocument();
			ApplyTheme();
			RefreshDiagnostics();
		}
		catch (Exception)
		{
		}
	}

	private void MainWindow_Activated(object sender, WindowActivatedEventArgs args)
	{
		ReloadIfChanged();
	}

	private static void SelectCombo(ComboBox combo, string tag)
	{
		foreach (ComboBoxItem item in combo.Items.OfType<ComboBoxItem>())
		{
			if (item.Tag is string text && text == tag)
			{
				combo.SelectedItem = item;
				return;
			}
		}
		combo.SelectedIndex = 0;
	}

	private static void SetSlider(Slider slider, float value)
	{
		slider.Value = Math.Clamp(value, slider.Minimum, slider.Maximum);
	}

	private void ApplyTheme()
	{
		string theme = _doc.Theme;
		Grid rootGrid = RootGrid;
		ElementTheme requestedTheme = ((theme == "light") ? ElementTheme.Light : ((theme == "dark") ? ElementTheme.Dark : ElementTheme.Default));
		rootGrid.RequestedTheme = requestedTheme;
		bool dark = theme == "dark" || (!(theme == "light") && !IsSystemLight());
		_dark = dark;
		byte a = (byte)(_micaAvailable ? 238 : byte.MaxValue);
		RootGrid.Background = new SolidColorBrush(_dark ? Color.FromArgb(a, 18, 18, 23) : Color.FromArgb(a, 247, 249, 252));
		ApplyAccentResources();
		BuildAccentSwatches();
		UpdateTitleBarColors();
	}

	private void ApplyAccentResources()
	{
		Color color = AccentFromId(_doc.Accent, _dark);
		RootGrid.Resources["SystemAccentColor"] = color;
		RootGrid.Resources["AccentFillColorDefaultBrush"] = new SolidColorBrush(color);
		RootGrid.Resources["AccentTextFillColorPrimaryBrush"] = new SolidColorBrush(color);
		RootGrid.Resources["AccentFillColorSecondaryBrush"] = new SolidColorBrush(Color.FromArgb(230, color.R, color.G, color.B));
		RootGrid.Resources["AccentFillColorTertiaryBrush"] = new SolidColorBrush(Color.FromArgb(160, color.R, color.G, color.B));
	}

	private void BuildAccentSwatches()
	{
		if ((object)AccentPanel != null)
		{
			AccentPanel.Children.Clear();
			(string, string)[] accentColors = AccentColors;
			for (int i = 0; i < accentColors.Length; i++)
			{
				(string, string) tuple = accentColors[i];
				string item = tuple.Item1;
				string item2 = tuple.Item2;
				Color color = AccentFromId(item, _dark);
				bool flag = string.Equals(_doc.Accent, item, StringComparison.OrdinalIgnoreCase);
				Color color2 = (_dark ? Color.FromArgb(byte.MaxValue, 12, 24, 32) : Color.FromArgb(byte.MaxValue, byte.MaxValue, byte.MaxValue, byte.MaxValue));
				Button button = new Button
				{
					Width = 32.0,
					Height = 32.0,
					Padding = new Thickness(0.0),
					CornerRadius = new CornerRadius(16.0),
					Background = new SolidColorBrush(color),
					BorderThickness = new Thickness(flag ? 3 : 0),
					BorderBrush = new SolidColorBrush(color2),
					Content = (flag ? new TextBlock
					{
						Text = "✓",
						FontSize = 14.0,
						FontWeight = FontWeights.SemiBold,
						Foreground = new SolidColorBrush(color2),
						HorizontalAlignment = HorizontalAlignment.Center,
						VerticalAlignment = VerticalAlignment.Center
					} : null),
					Tag = item
				};
				ToolTipService.SetToolTip(button, item2);
				button.Click += AccentButton_Click;
				AccentPanel.Children.Add(button);
			}
		}
	}

	private void UpdateTitleBarColors()
	{
		if ((object)_appWindow != null)
		{
			Color value = (_dark ? Color.FromArgb(byte.MaxValue, byte.MaxValue, byte.MaxValue, byte.MaxValue) : Color.FromArgb(byte.MaxValue, 0, 0, 0));
			Color value2 = (_dark ? Color.FromArgb(120, byte.MaxValue, byte.MaxValue, byte.MaxValue) : Color.FromArgb(120, 0, 0, 0));
			_appWindow.TitleBar.ButtonForegroundColor = value;
			_appWindow.TitleBar.ButtonInactiveForegroundColor = value2;
			Color value3 = (_dark ? Color.FromArgb(byte.MaxValue, 18, 18, 23) : Color.FromArgb(byte.MaxValue, 247, 249, 252));
			_appWindow.TitleBar.ButtonBackgroundColor = value3;
			_appWindow.TitleBar.ButtonInactiveBackgroundColor = value3;
		}
	}

	private static bool IsSystemLight()
	{
		try
		{
			using RegistryKey registryKey = Registry.CurrentUser.OpenSubKey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
			return !(registryKey?.GetValue("AppsUseLightTheme") is int num) || num != 0;
		}
		catch
		{
			return true;
		}
	}

	private static Color AccentFromId(string id, bool dark)
	{
		return id.ToLowerInvariant() switch
		{
			"teal" => dark ? Color.FromArgb(byte.MaxValue, 45, 212, 191) : Color.FromArgb(byte.MaxValue, 15, 118, 110), 
			"violet" => dark ? Color.FromArgb(byte.MaxValue, 182, 156, byte.MaxValue) : Color.FromArgb(byte.MaxValue, 109, 40, 217), 
			"orange" => dark ? Color.FromArgb(byte.MaxValue, byte.MaxValue, 164, 92) : Color.FromArgb(byte.MaxValue, 194, 65, 12), 
			"pink" => dark ? Color.FromArgb(byte.MaxValue, byte.MaxValue, 143, 199) : Color.FromArgb(byte.MaxValue, 190, 24, 93), 
			_ => dark ? Color.FromArgb(byte.MaxValue, 96, 205, byte.MaxValue) : Color.FromArgb(byte.MaxValue, 0, 103, 192), 
		};
	}

	private void Slider_ValueChanged(object sender, RangeBaseValueChangedEventArgs e)
	{
		if (!_loading && sender is Slider { Tag: string tag })
		{
			float num = (float)e.NewValue;
			switch (tag)
			{
			case "icon_size":
				_doc.IconSize = MathF.Round(num);
				break;
			case "spacing":
				_doc.Spacing = MathF.Round(num);
				break;
			case "padding":
				_doc.Padding = MathF.Round(num);
				break;
			case "corner_radius":
				_doc.CornerRadius = MathF.Round(num);
				break;
			case "opacity":
				_doc.Opacity = Math.Clamp(num / 100f, 0.2f, 1f);
				break;
			case "edge_margin":
				_doc.EdgeMargin = MathF.Round(num);
				break;
			case "offset":
				_doc.MonitorOffset = MathF.Round(num);
				break;
			}
			OnChanged();
		}
	}

	private void Toggle_Toggled(object sender, RoutedEventArgs e)
	{
		if (!_loading && sender is ToggleSwitch { Tag: string tag } toggleSwitch)
		{
			switch (tag)
			{
			default:
				return;
			case "magnification":
				_doc.Magnification = toggleSwitch.IsOn;
				UpdateMagnificationEnabled();
				break;
			case "tooltips":
				_doc.ShowTooltips = toggleSwitch.IsOn;
				break;
			case "launch_animation":
				_doc.LaunchAnimation = toggleSwitch.IsOn;
				break;
			case "running_indicator":
				_doc.ShowRunningIndicator = toggleSwitch.IsOn;
				break;
			}
			OnChanged();
		}
	}

	private StartupMode SelectedStartupMode()
	{
		return StartupModeChoices.SelectedIndex == 1 ? StartupMode.Run : StartupMode.Task;
	}

	private void SelectStartupMode(StartupMode mode)
	{
		StartupModeChoices.SelectedIndex = ((mode == StartupMode.Run) ? 1 : 0);
	}

	private void RefreshStartupUi(string notice = null, StartupMode? selected = null)
	{
		_startupSyncing = true;
		try
		{
			StartupState startupState = _startup.Query();
			StartupMode activeMode = startupState.Task.Enabled
				? StartupMode.Task
				: startupState.RunExists
					? StartupMode.Run
					: (selected ?? _startup.PreferredMode);
			SelectStartupMode(selected ?? activeMode);
			StartupToggle.IsOn = startupState.Task.Enabled || startupState.RunExists;
			StartupToggle.IsEnabled = !_startupBusy && App.DockExecutablePath.Length > 0;
			StartupModeChoices.IsEnabled = !_startupBusy;
			StartupHintText.Text = App.DockExecutablePath.Length == 0
				? "找不到 glide-dock.exe；请从 Dock 的“设置”入口打开。"
				: (selected == StartupMode.Run || activeMode == StartupMode.Run
					? "兼容模式写入当前用户 Run 项，无需管理员权限；Windows 可能延迟启动。"
					: "快速模式创建当前用户的登录计划任务，登录后尽早启动；首次设置需要管理员授权。");

			if (!string.IsNullOrWhiteSpace(notice))
			{
				StartupCard.Description = notice;
			}
			else if (startupState.Task.Exists && startupState.RunExists)
			{
				StartupCard.Description = "检测到计划任务和 Run 同时存在；关闭自启动会一起清理。";
			}
			else if (!startupState.Task.Enabled && !startupState.RunExists)
			{
				StartupCard.Description = startupState.Task.Exists
					? "计划任务已被禁用；重新开启可恢复。"
					: "未启用；登录 Windows 时不会自动启动。";
			}
			else
			{
				string registered = StartupManager.RegisteredExecutable(startupState);
				if (!string.IsNullOrWhiteSpace(registered)
					&& App.DockExecutablePath.Length > 0
					&& !string.Equals(Path.GetFullPath(registered), Path.GetFullPath(App.DockExecutablePath), StringComparison.OrdinalIgnoreCase))
				{
					StartupCard.Description = "启动项指向其他位置；关闭再开启可更新到当前目录。";
				}
				else
				{
					StartupCard.Description = startupState.Task.Enabled
						? "已启用快速模式（计划任务）。"
						: "已启用兼容模式（注册表 Run）。";
				}
			}
		}
		catch (Exception ex)
		{
			StartupToggle.IsOn = false;
			StartupToggle.IsEnabled = false;
			StartupModeChoices.IsEnabled = false;
			StartupCard.Description = "无法读取开机启动状态：" + ex.Message;
		}
		finally
		{
			_startupSyncing = false;
		}
	}

	private void StartupToggle_Toggled(object sender, RoutedEventArgs e)
	{
		if (_loading || _startupBusy || _startupSyncing || !(sender is ToggleSwitch { IsOn: var isOn } toggleSwitch))
		{
			return;
		}
		_startupBusy = true;
		StartupToggle.IsEnabled = false;
		StartupModeChoices.IsEnabled = false;
		try
		{
			if (isOn)
			{
				string dockExecutablePath = App.DockExecutablePath;
				if (string.IsNullOrWhiteSpace(dockExecutablePath) || !File.Exists(dockExecutablePath))
				{
					throw new FileNotFoundException("找不到 glide-dock.exe，请从 Dock 的“设置”入口打开。");
				}
				string notice = _startup.Enable(SelectedStartupMode(), dockExecutablePath);
				_startupBusy = false;
				RefreshStartupUi(notice, SelectedStartupMode());
			}
			else
			{
				_startup.DisableAll();
				_startupBusy = false;
				RefreshStartupUi();
			}
		}
		catch (Exception ex)
		{
			_startupBusy = false;
			_loading = true;
			toggleSwitch.IsOn = !isOn;
			_loading = false;
			RefreshStartupUi("设置开机自启动失败：" + ex.Message);
		}
	}

	private void StartupModeChoices_SelectionChanged(object sender, SelectionChangedEventArgs e)
	{
		if (_loading || _startupBusy || _startupSyncing)
		{
			return;
		}
		StartupMode startupMode = SelectedStartupMode();
		var currentState = _startup.Query();
		if ((startupMode == StartupMode.Task && currentState.Task.Enabled)
			|| (startupMode == StartupMode.Run && currentState.RunExists))
		{
			// Loading the page selects the already-active mode; do not create
			// a second UAC prompt or migration just because of that selection.
			_startup.SetPreferredMode(startupMode);
			RefreshStartupUi();
			return;
		}
		if (!StartupToggle.IsOn)
		{
			_startup.SetPreferredMode(startupMode);
			RefreshStartupUi();
			return;
		}
		_startupBusy = true;
		StartupToggle.IsEnabled = false;
		StartupModeChoices.IsEnabled = false;
		try
		{
			string dockExecutablePath = App.DockExecutablePath;
			if (string.IsNullOrWhiteSpace(dockExecutablePath) || !File.Exists(dockExecutablePath))
			{
				throw new FileNotFoundException("找不到 glide-dock.exe，无法切换启动方式。");
			}
			string notice = _startup.Enable(startupMode, dockExecutablePath);
			_startupBusy = false;
			RefreshStartupUi(notice, startupMode);
		}
		catch (Exception ex)
		{
			_startupBusy = false;
			StartupMode mode = (_startup.Query().Task.Enabled ? StartupMode.Task : StartupMode.Run);
			_loading = true;
			SelectStartupMode(mode);
			_loading = false;
			RefreshStartupUi("切换启动方式失败：" + ex.Message);
		}
	}

	private void UpdateMagnificationEnabled()
	{
		bool isOn = MagnificationToggle.IsOn;
		MaxScaleSlider.IsEnabled = isOn;
		RangeSlider.IsEnabled = isOn;
	}

	private void MotionSlider_ValueChanged(object sender, RangeBaseValueChangedEventArgs e)
	{
		if (!_loading && sender is Slider { Tag: string tag })
		{
			float num = (float)e.NewValue;
			switch (tag)
			{
			case "max_scale":
				_doc.MaxScale = MathF.Round(num, 2);
				break;
			case "hover_ms":
				_doc.HoverMilliseconds = (int)MathF.Round(num);
				break;
			case "elasticity":
				_doc.Elasticity = Math.Clamp(num / 100f, 0f, 1f);
				break;
			case "range_slots":
				_doc.RangeSlots = MathF.Round(num, 1);
				break;
			}
			OnChanged();
		}
	}

	private void MotionToggle_Toggled(object sender, RoutedEventArgs e)
	{
		if (_loading || !(sender is ToggleSwitch { Tag: string tag } toggleSwitch))
		{
			return;
		}
		if (!(tag == "motion_enabled"))
		{
			if (!(tag == "respect_reduced"))
			{
				return;
			}
			_doc.RespectReducedMotion = toggleSwitch.IsOn;
		}
		else
		{
			_doc.MotionEnabled = toggleSwitch.IsOn;
		}
		OnChanged();
	}

	private void HoverPresetCombo_SelectionChanged(object sender, SelectionChangedEventArgs e)
	{
		if (!_loading && HoverPresetCombo.SelectedItem is ComboBoxItem { Tag: string tag })
		{
			_doc.HoverPreset = tag;
			UpdateRecommendation();
			UpdateClassicDurationState();
			OnChanged();
		}
	}

	private void UpdateClassicDurationState()
	{
		bool flag = HoverPresetCombo.SelectedItem is ComboBoxItem { Tag: string tag } && tag == "glide_zoom";
		HoverMsSlider.IsEnabled = !flag;
		HoverMsValue.Opacity = (flag ? 0.45 : 1.0);
	}

	private void LaunchPresetCombo_SelectionChanged(object sender, SelectionChangedEventArgs e)
	{
		if (!_loading && LaunchPresetCombo.SelectedItem is ComboBoxItem { Tag: string tag })
		{
			_doc.LaunchPreset = tag;
			OnChanged();
		}
	}

	private void CopyMotion_Click(object sender, RoutedEventArgs e)
	{
		try
		{
			DataPackage dataPackage = new DataPackage();
			dataPackage.SetText(_doc.MotionJson());
			Clipboard.SetContent(dataPackage);
		}
		catch (Exception)
		{
		}
	}

	private void RefreshDiagnostics_Click(object sender, RoutedEventArgs e)
	{
		RefreshDiagnostics();
	}

	private void RefreshDiagnostics()
	{
		try
		{
			_lastDiagnostics = DiagnosticsService.Collect(_doc.Path, _startup.PreferredMode);
			DiagnosticsSummary.Text = _lastDiagnostics.Summary;
			DiagnosticsSummary.Foreground = new SolidColorBrush(
				_lastDiagnostics.HasWarning ? Colors.Orange : Colors.Green);
			DiagnosticsHint.Text = _lastDiagnostics.HasWarning
				? "请按下列原因处理；诊断只读取状态，不会自动修改系统。"
				: "未发现异常。如仍无法拖放，请重启 Dock 后重试，或打开日志目录反馈。";
			DiagnosticsRows.Children.Clear();
			foreach ((string label, string value) in _lastDiagnostics.Rows)
			{
				Grid row = new Grid
				{
					ColumnSpacing = 12
				};
				row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(140) });
				row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
				TextBlock labelBlock = new TextBlock
				{
					Text = label,
					FontWeight = FontWeights.SemiBold,
					VerticalAlignment = VerticalAlignment.Top
				};
				TextBlock valueBlock = new TextBlock
				{
					Text = value,
					TextWrapping = TextWrapping.Wrap,
					MaxWidth = 520,
					Opacity = 0.82
				};
				Grid.SetColumn(labelBlock, 0);
				Grid.SetColumn(valueBlock, 1);
				row.Children.Add(labelBlock);
				row.Children.Add(valueBlock);
				DiagnosticsRows.Children.Add(row);
			}
		}
		catch (Exception error)
		{
			_lastDiagnostics = null;
			DiagnosticsSummary.Text = "诊断失败：" + error.Message;
			DiagnosticsSummary.Foreground = new SolidColorBrush(Colors.OrangeRed);
			DiagnosticsHint.Text = "请打开日志目录查看错误详情。";
			DiagnosticsRows.Children.Clear();
		}
	}

	private void CopyDiagnostics_Click(object sender, RoutedEventArgs e)
	{
		if (_lastDiagnostics is null)
		{
			RefreshDiagnostics();
		}
		if (_lastDiagnostics is null)
		{
			return;
		}
		try
		{
			DataPackage dataPackage = new DataPackage();
			dataPackage.SetText(_lastDiagnostics.PlainText);
			Clipboard.SetContent(dataPackage);
		}
		catch (Exception)
		{
		}
	}

	private void OpenLogs_Click(object sender, RoutedEventArgs e)
	{
		try
		{
			string directory = Path.Combine(
				Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
				"Glide-Dock",
				"logs");
			Directory.CreateDirectory(directory);
			Process.Start(new ProcessStartInfo
			{
				FileName = directory,
				UseShellExecute = true
			});
		}
		catch (Exception error)
		{
			DiagnosticsSummary.Text = "无法打开日志目录：" + error.Message;
		}
	}

	private void UpdateRecommendation()
	{
		(string, string) tuple = _doc.HoverPreset switch
		{
			"bounce" => ("Triple Bounce", "triple_bounce"), 
			"wave" => ("Ripple Launch", "ripple_launch"), 
			"glow" => ("Pulse Launch", "pulse_launch"), 
			_ => ("Glide Launch", "glide_launch"), 
		};
		RecommendationText.Text = "推荐搭配：" + tuple.Item1;
	}

	private void ThemeCombo_SelectionChanged(object sender, SelectionChangedEventArgs e)
	{
		if (!_loading && ThemeCombo.SelectedItem is ComboBoxItem { Tag: string tag })
		{
			_doc.Theme = tag;
			ApplyTheme();
			OnChanged();
		}
	}

	private void LayerCombo_SelectionChanged(object sender, SelectionChangedEventArgs e)
	{
		if (!_loading && LayerCombo.SelectedItem is ComboBoxItem { Tag: string tag })
		{
			_doc.Layer = tag;
			OnChanged();
		}
	}

	private void AccentButton_Click(object sender, RoutedEventArgs e)
	{
		if (!_loading && sender is Button { Tag: string tag })
		{
			_doc.Accent = tag;
			ApplyTheme();
			OnChanged();
		}
	}

	private void Shortcut_Click(object sender, RoutedEventArgs e)
	{
		if (sender is Button { Tag: string tag })
		{
			SelectPage(tag);
		}
	}

	private void OpenConfig_Click(object sender, RoutedEventArgs e)
	{
		try
		{
			string directoryName = Path.GetDirectoryName(_doc.Path);
			if (!string.IsNullOrEmpty(directoryName))
			{
				Directory.CreateDirectory(directoryName);
				Process.Start(new ProcessStartInfo
				{
					FileName = directoryName,
					UseShellExecute = true
				});
			}
		}
		catch (Exception)
		{
		}
	}

	private void Reload_Click(object sender, RoutedEventArgs e)
	{
		try
		{
			_doc.Reload();
			LoadFromDocument();
			ApplyTheme();
			OnChanged();
		}
		catch (Exception)
		{
		}
	}

	private void Reset_Click(object sender, RoutedEventArgs e)
	{
		_doc.ResetDefaults();
		LoadFromDocument();
		ApplyTheme();
		OnChanged();
	}

	private void OnChanged()
	{
		if (_loading)
		{
			return;
		}
		try
		{
			bool flag = _doc.Send(_owner, WindowNative.GetWindowHandle(this));
			if (!AttachedToDock || flag)
			{
				_saveTimer.Stop();
				_saveTimer.Start();
			}
		}
		catch (Exception)
		{
		}
	}

	private void SaveNow()
	{
		if (AttachedToDock)
		{
			return;
		}
		try
		{
			_saving = true;
			_doc.Save();
			_lastKnownWriteUtc = ReadConfigWriteTime();
		}
		catch (Exception)
		{
		}
		finally
		{
			_saving = false;
		}
	}

	private void NavView_SelectionChanged(NavigationView sender, NavigationViewSelectionChangedEventArgs args)
	{
		if (args.SelectedItem is NavigationViewItem { Tag: string tag })
		{
			if (!string.IsNullOrEmpty(SearchBox.Text))
			{
				_loading = true;
				SearchBox.Text = string.Empty;
				_loading = false;
			}
			ShowPage(tag);
		}
	}

	private void SelectPage(string tag)
	{
		foreach (NavigationViewItem item in NavView.MenuItems.OfType<NavigationViewItem>())
		{
			if (item.Tag is string text && text == tag)
			{
				NavView.SelectedItem = item;
				break;
			}
		}
		ShowPage(tag);
	}

	private void ShowPage(string tag)
	{
		_currentPage = tag;
		GeneralPage.Visibility = ((!(tag == "general")) ? Visibility.Collapsed : Visibility.Visible);
		AppearancePage.Visibility = ((!(tag == "appearance")) ? Visibility.Collapsed : Visibility.Visible);
		BehaviorPage.Visibility = ((!(tag == "behavior")) ? Visibility.Collapsed : Visibility.Visible);
		AnimationPage.Visibility = ((!(tag == "animation")) ? Visibility.Collapsed : Visibility.Visible);
		DiagnosticsPage.Visibility = ((!(tag == "diagnostics")) ? Visibility.Collapsed : Visibility.Visible);
		AboutPage.Visibility = ((!(tag == "about")) ? Visibility.Collapsed : Visibility.Visible);
		SearchPage.Visibility = Visibility.Collapsed;
		if (PageInfo.TryGetValue(tag, out (string, string) value))
		{
			PageTitle.Text = value.Item1;
			PageSubtitle.Text = value.Item2;
		}
		if (tag == "diagnostics")
		{
			RefreshDiagnostics();
		}
		ContentScroll.ChangeView(null, 0.0, null);
		AnimatePageTransition();
	}

	private void AnimatePageTransition()
	{
		PageHost.Opacity = 0.0;
		PageTransform.Y = 10.0;
		DoubleAnimation fade = new DoubleAnimation
		{
			To = 1.0,
			Duration = new Duration(TimeSpan.FromMilliseconds(180.0)),
			EasingFunction = new CubicEase { EasingMode = EasingMode.EaseOut }
		};
		Storyboard.SetTarget(fade, PageHost);
		Storyboard.SetTargetProperty(fade, "Opacity");
		DoubleAnimation slide = new DoubleAnimation
		{
			To = 0.0,
			Duration = new Duration(TimeSpan.FromMilliseconds(220.0)),
			EasingFunction = new CubicEase { EasingMode = EasingMode.EaseOut }
		};
		Storyboard.SetTarget(slide, PageTransform);
		Storyboard.SetTargetProperty(slide, "Y");
		Storyboard storyboard = new Storyboard();
		storyboard.Children.Add(fade);
		storyboard.Children.Add(slide);
		storyboard.Begin();
	}

	private void OpenDiagnostics_Click(object sender, RoutedEventArgs e)
	{
		SelectPage("diagnostics");
	}

	private void BuildSearchIndex()
	{
		_searchIndex.Clear();
		_searchIndex.AddRange(new SearchEntry[33]
		{
			new SearchEntry("主题", "深色、浅色或跟随系统。", "general", "theme 深色 浅色 系统 模式"),
			new SearchEntry("强调色", "蓝色、青绿、紫罗兰、暖橙、玫瑰。", "general", "accent 颜色 强调色"),
			new SearchEntry("配置文件", "打开配置文件夹或重新加载 JSON。", "general", "config json 文件 路径"),
			new SearchEntry("恢复默认设置", "重置外观与行为，不删除条目。", "general", "reset 重置 默认"),
			new SearchEntry("Dock 层级", "桌面同级、动态层级或置顶。", "general", "layer z-order topmost 层级 置顶 桌面"),
			new SearchEntry("图标大小", "单个图标的边长。", "appearance", "icon size 图标 尺寸"),
			new SearchEntry("图标间距", "相邻图标之间的空隙。", "appearance", "spacing gap 间距"),
			new SearchEntry("内边距", "玻璃背景到图标的距离。", "appearance", "padding 内边距"),
			new SearchEntry("圆角", "Dock 背景圆角。", "appearance", "radius corner 圆角"),
			new SearchEntry("背景不透明度", "玻璃背景的透明程度。", "appearance", "opacity 透明 不透明度"),
			new SearchEntry("离屏幕底边距离", "Dock 与工作区底边的距离。", "appearance", "edge margin 底边 距离"),
			new SearchEntry("水平偏移", "左右微调 Dock 位置。", "appearance", "offset 水平 偏移"),
			new SearchEntry("悬停动效", "开启或关闭悬停层动画。", "behavior", "magnification hover 放大"),
			new SearchEntry("名称提示", "悬停显示应用名称。", "behavior", "tooltip 提示 名称"),
			new SearchEntry("启动动画", "启动后播放所选启动动画。", "behavior", "launch animation 回弹 启动"),
			new SearchEntry("运行状态圆点", "显示运行应用的小圆点。", "behavior", "running indicator 圆点 状态"),
			new SearchEntry("开机自启动", "登录 Windows 时自动启动 Dock。", "general", "startup autostart 开机 启动 登录 自启"),
			new SearchEntry("启动方式", "快速计划任务，或无需管理员的兼容 Run。", "general", "startup mode task scheduler run 计划任务 注册表 兼容"),
			new SearchEntry("启用动画", "关闭后所有图标动效停止。", "animation", "motion enable 动画 关闭"),
			new SearchEntry("减少动态效果", "跟随 Windows 的无动画设置。", "animation", "reduced motion 减少 动态"),
			new SearchEntry("悬停动画", "Glide Zoom、Bounce、Wave、Fluid Press、Glow、Perspective。", "animation", "hover preset 悬停 动画"),
			new SearchEntry("启动动画预设", "Glide Launch、Triple Bounce、Ripple Launch、Pulse Launch。", "animation", "launch preset 启动 动画"),
			new SearchEntry("最大放大倍率", "悬停放大的峰值。", "animation", "max scale 放大 倍率"),
			new SearchEntry("动画时长", "悬停层基准时长。", "animation", "duration 时长 ms"),
			new SearchEntry("弹性强度", "弹簧的弹性程度。", "animation", "elasticity 弹性"),
			new SearchEntry("影响范围", "放大向两侧扩散的槽位。", "animation", "range slots 范围"),
			new SearchEntry("复制动画配置", "把 motion 配置复制到剪贴板。", "animation", "copy json 复制 配置"),
			new SearchEntry("拖拽诊断", "检查 Dock、设置中心、Explorer 的权限与拖放状态。", "diagnostics", "drag drop 拖拽 诊断 权限 管理员 拖不进"),
			new SearchEntry("权限诊断", "检查管理员权限、UIPI、Low Integrity。", "diagnostics", "permission uac integrity 管理员 权限 完整性"),
			new SearchEntry("守护进程", "检查隐藏 guard 是否在运行。", "diagnostics", "guard daemon 守护 进程"),
			new SearchEntry("UAC 与兼容性", "查看 UAC 注册表和 AppCompatFlags。", "diagnostics", "uac compat 兼容性 启动"),
			new SearchEntry("版本", "当前 Glide-Dock 版本。", "about", "version 版本"),
			new SearchEntry("许可证", "MIT OR Apache-2.0。", "about", "license 许可证")
		});
	}

	private void SearchBox_TextChanged(AutoSuggestBox sender, AutoSuggestBoxTextChangedEventArgs args)
	{
		if (!_loading)
		{
			string text = sender.Text.Trim();
			if (text.Length == 0)
			{
				ShowPage(_currentPage);
			}
			else
			{
				ShowSearch(text);
			}
		}
	}

	private void SearchBox_QuerySubmitted(AutoSuggestBox sender, AutoSuggestBoxQuerySubmittedEventArgs args)
	{
		string text = args.QueryText.Trim();
		if (text.Length > 0)
		{
			ShowSearch(text);
		}
	}

	private void ShowSearch(string query)
	{
		string query2 = query;
		GeneralPage.Visibility = Visibility.Collapsed;
		AppearancePage.Visibility = Visibility.Collapsed;
		BehaviorPage.Visibility = Visibility.Collapsed;
		AnimationPage.Visibility = Visibility.Collapsed;
		DiagnosticsPage.Visibility = Visibility.Collapsed;
		AboutPage.Visibility = Visibility.Collapsed;
		SearchPage.Visibility = Visibility.Visible;
		PageTitle.Text = "搜索";
		PageSubtitle.Text = "在设置中查找“" + query2 + "”";
		ContentScroll.ChangeView(null, 0.0, null);
		List<SearchEntry> list = _searchIndex.Where((SearchEntry entry) => Matches(entry, query2)).ToList();
		SearchSummary.Text = $"找到 {list.Count} 项设置";
		SearchEmptyPanel.Visibility = ((list.Count != 0) ? Visibility.Collapsed : Visibility.Visible);
		SearchResultsPanel.Children.Clear();
		foreach (SearchEntry item in list)
		{
			SearchResultsPanel.Children.Add(CreateSearchResult(item));
		}
		AnimatePageTransition();
	}

	private static bool Matches(SearchEntry entry, string query)
	{
		if (!entry.Title.Contains(query, StringComparison.OrdinalIgnoreCase) && !entry.Description.Contains(query, StringComparison.OrdinalIgnoreCase))
		{
			return entry.Keywords.Contains(query, StringComparison.OrdinalIgnoreCase);
		}
		return true;
	}

	private Button CreateSearchResult(SearchEntry entry)
	{
		SymbolIcon item = new SymbolIcon
		{
			Symbol = Symbol.Setting,
			Foreground = new SolidColorBrush(AccentFromId(_doc.Accent, _dark))
		};
		TextBlock item2 = new TextBlock
		{
			Text = entry.Title,
			FontSize = 14.0,
			FontWeight = FontWeights.SemiBold
		};
		TextBlock item3 = new TextBlock
		{
			Text = entry.Description,
			FontSize = 12.0,
			Foreground = new SolidColorBrush(_dark ? Color.FromArgb(byte.MaxValue, 180, 180, 190) : Color.FromArgb(byte.MaxValue, 96, 96, 106))
		};
		StackPanel stackPanel = new StackPanel
		{
			Spacing = 2.0
		};
		stackPanel.Children.Add(item2);
		stackPanel.Children.Add(item3);
		TextBlock textBlock = new TextBlock();
		string text;
		if (PageInfo.TryGetValue(entry.PageTag, out (string, string) value))
		{
			(text, _) = value;
		}
		else
		{
			text = string.Empty;
		}
		textBlock.Text = text;
		textBlock.FontSize = 12.0;
		textBlock.Foreground = new SolidColorBrush(_dark ? Color.FromArgb(byte.MaxValue, 180, 180, 190) : Color.FromArgb(byte.MaxValue, 96, 96, 106));
		textBlock.VerticalAlignment = VerticalAlignment.Center;
		TextBlock textBlock2 = textBlock;
		Grid grid = new Grid
		{
			ColumnSpacing = 12.0
		};
		grid.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = GridLength.Auto
		});
		grid.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = new GridLength(1.0, GridUnitType.Star)
		});
		grid.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = GridLength.Auto
		});
		Grid.SetColumn(stackPanel, 1);
		Grid.SetColumn(textBlock2, 2);
		grid.Children.Add(item);
		grid.Children.Add(stackPanel);
		grid.Children.Add(textBlock2);
		Button obj = new Button
		{
			Tag = entry.PageTag,
			Content = grid,
			HorizontalContentAlignment = HorizontalAlignment.Stretch,
			Padding = new Thickness(16.0),
			Margin = new Thickness(0.0, 0.0, 0.0, 6.0),
			CornerRadius = new CornerRadius(8.0)
		};
		AutomationProperties.SetName(obj, entry.Title);
		obj.Click += SearchResult_Click;
		return obj;
	}

	private void SearchResult_Click(object sender, RoutedEventArgs e)
	{
		if (sender is Button { Tag: string tag })
		{
			_loading = true;
			SearchBox.Text = string.Empty;
			_loading = false;
			SelectPage(tag);
		}
	}

}
