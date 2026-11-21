using GlideDock.Settings.Controls;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;

namespace GlideDock.Settings;

internal static class SettingsTypography
{
    // Walk authored content only. Visual-tree traversal enters control templates
    // and replaces the symbol font on their internal glyph TextBlocks.
    public static void Apply(object? content, FontFamily family)
    {
        if (content is IconElement) return;
        if (content is TextBlock text) text.FontFamily = family;
        if (content is Control control) control.FontFamily = family;

        if (content is NavigationView navigation)
        {
            foreach (var item in navigation.MenuItems) Apply(item, family);
            foreach (var item in navigation.FooterMenuItems) Apply(item, family);
        }
        if (content is SettingsCard card)
        {
            Apply(card.CardContent, family);
            Apply(card.ExpanderContent, family);
        }

        switch (content)
        {
            case Panel panel:
                foreach (var child in panel.Children) Apply(child, family);
                break;
            case Border border:
                Apply(border.Child, family);
                break;
            case ContentControl container:
                Apply(container.Content, family);
                break;
            case ContentPresenter presenter:
                Apply(presenter.Content, family);
                break;
            case ItemsControl items:
                foreach (var item in items.Items) Apply(item, family);
                break;
        }
    }
}
