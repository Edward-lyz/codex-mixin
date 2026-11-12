using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;

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
            case HeaderedItemsControl item:
                if (item.Header is string header) item.Header = Localized.Get(header);
                foreach (var child in item.Items) Translate(child);
                break;
            case ItemsControl items: foreach (var child in items.Items) Translate(child); break;
            case Panel panel: foreach (var child in panel.Children) Translate(child); break;
            case Border border: Translate(border.Child); break;
            case HeaderedContentControl tab:
                if (tab.Header is string title) tab.Header = Localized.Get(title);
                Translate(tab.Content);
                break;
            case ContentControl control:
                if (control.Content is string content) control.Content = Localized.Get(content);
                else Translate(control.Content);
                Translate(control.ContextMenu);
                break;
        }
    }
}
