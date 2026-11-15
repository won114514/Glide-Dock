using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media.Animation;
using System;

namespace GlideDock.Settings.Controls;

public sealed partial class SettingsCard : UserControl
{
    public static readonly DependencyProperty TitleProperty = DependencyProperty.Register(
        nameof(Title), typeof(string), typeof(SettingsCard), new PropertyMetadata(string.Empty));

    public static readonly DependencyProperty DescriptionProperty = DependencyProperty.Register(
        nameof(Description), typeof(string), typeof(SettingsCard), new PropertyMetadata(string.Empty));

    public static readonly DependencyProperty IconSymbolProperty = DependencyProperty.Register(
        nameof(IconSymbol), typeof(Symbol), typeof(SettingsCard), new PropertyMetadata(Symbol.Setting));

    public static readonly DependencyProperty CardContentProperty = DependencyProperty.Register(
        nameof(CardContent), typeof(object), typeof(SettingsCard), new PropertyMetadata(null));

    public static readonly DependencyProperty IsExpandableProperty = DependencyProperty.Register(
        nameof(IsExpandable), typeof(bool), typeof(SettingsCard),
        new PropertyMetadata(false, OnExpansionPropertiesChanged));

    public static readonly DependencyProperty IsExpandedProperty = DependencyProperty.Register(
        nameof(IsExpanded), typeof(bool), typeof(SettingsCard),
        new PropertyMetadata(false, OnExpansionPropertiesChanged));

    public static readonly DependencyProperty ExpanderContentProperty = DependencyProperty.Register(
        nameof(ExpanderContent), typeof(object), typeof(SettingsCard), new PropertyMetadata(null));

    public SettingsCard()
    {
        InitializeComponent();
        UpdateExpansion();
    }

    public string Title
    {
        get => (string)GetValue(TitleProperty);
        set => SetValue(TitleProperty, value);
    }

    public string Description
    {
        get => (string)GetValue(DescriptionProperty);
        set => SetValue(DescriptionProperty, value);
    }

    public Symbol IconSymbol
    {
        get => (Symbol)GetValue(IconSymbolProperty);
        set => SetValue(IconSymbolProperty, value);
    }

    public object? CardContent
    {
        get => GetValue(CardContentProperty);
        set => SetValue(CardContentProperty, value);
    }

    public bool IsExpandable
    {
        get => (bool)GetValue(IsExpandableProperty);
        set => SetValue(IsExpandableProperty, value);
    }

    public bool IsExpanded
    {
        get => (bool)GetValue(IsExpandedProperty);
        set => SetValue(IsExpandedProperty, value);
    }

    public object? ExpanderContent
    {
        get => GetValue(ExpanderContentProperty);
        set => SetValue(ExpanderContentProperty, value);
    }

    private static void OnExpansionPropertiesChanged(
        DependencyObject dependencyObject,
        DependencyPropertyChangedEventArgs args)
    {
        if (dependencyObject is SettingsCard card)
        {
            card.UpdateExpansion();
        }
    }

    private void UpdateExpansion()
    {
        if (ExpansionHost is null || ExpandButton is null || ExpandIcon is null)
        {
            return;
        }

        ExpandButton.Visibility = IsExpandable ? Visibility.Visible : Visibility.Collapsed;
        bool expanded = IsExpandable && IsExpanded;
        if (expanded && ExpansionHost.Visibility != Visibility.Visible)
        {
            ExpansionHost.Visibility = Visibility.Visible;
            ExpansionHost.Opacity = 0.0;
            DoubleAnimation fade = new DoubleAnimation
            {
                To = 1.0,
                Duration = new Duration(TimeSpan.FromMilliseconds(180.0)),
                EasingFunction = new CubicEase { EasingMode = EasingMode.EaseOut }
            };
            Storyboard.SetTarget(fade, ExpansionHost);
            Storyboard.SetTargetProperty(fade, "Opacity");
            Storyboard storyboard = new Storyboard();
            storyboard.Children.Add(fade);
            storyboard.Begin();
        }
        else if (!expanded)
        {
            ExpansionHost.Visibility = Visibility.Collapsed;
            ExpansionHost.Opacity = 0.0;
        }
        ExpandIcon.Glyph = IsExpanded ? "\uE70E" : "\uE70D";
        ToolTipService.SetToolTip(ExpandButton, IsExpanded ? "收起选项" : "展开选项");
    }

    private void ExpandButton_Click(object sender, RoutedEventArgs e)
    {
        IsExpanded = !IsExpanded;
    }
}
