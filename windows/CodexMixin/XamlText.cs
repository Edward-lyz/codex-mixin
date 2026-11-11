using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;

namespace CodexMixin;

// Translates the static text written in .xaml files after InitializeComponent.
internal static class XamlText
{
    internal static void Translate(object? node)
    {
        if (!Localized.UseEnglish || node is null) return;
        if (node is DependencyObject element)
        {
            var name = AutomationProperties.GetName(element);
            if (!string.IsNullOrEmpty(name)) AutomationProperties.SetName(element, Localized.Get(name));
            if (ToolTipService.GetToolTip(element) is string tip) ToolTipService.SetToolTip(element, Localized.Get(tip));
        }
        switch (node)
        {
            case TextBlock text: text.Text = Localized.Get(text.Text); break;
            case SelectorBarItem item: item.Text = Localized.Get(item.Text); break;
            case MenuFlyoutItem item: item.Text = Localized.Get(item.Text); break;
            case MenuFlyout flyout: foreach (var item in flyout.Items) Translate(item); break;
            case SelectorBar bar: foreach (var item in bar.Items) Translate(item); break;
            case Panel panel: foreach (var child in panel.Children) Translate(child); break;
            case Border border: Translate(border.Child); break;
            case ContentControl control:
                if (control.Content is string content) control.Content = Localized.Get(content);
                else Translate(control.Content);
                if (control is Button button) Translate(button.Flyout);
                break;
        }
    }
}
