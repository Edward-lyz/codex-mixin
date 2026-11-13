using System.ComponentModel;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;

namespace CodexMixin;

// Closing hides the retained result; only Dismiss releases the owned window.
internal sealed class OperationWindow : Window
{
    private const int OutputLimit = 131_072;
    private readonly TextBox output = new() { IsReadOnly = true, AcceptsReturn = true,
        TextWrapping = TextWrapping.Wrap, VerticalScrollBarVisibility = ScrollBarVisibility.Auto,
        FontFamily = new FontFamily("Cascadia Mono, Consolas"), FontSize = 12, BorderThickness = new Thickness(0) };
    private readonly TextBlock status = new() { TextWrapping = TextWrapping.Wrap };
    private readonly Button cancel;
    private readonly ProgressRing ring = new() { Width = 18, Height = 18, IsActive = true };
    private bool dismissing;
    internal bool IsRunning { get; private set; } = true;

    internal OperationWindow(Window owner, string title, Action onCancel)
    {
        Owner = owner; Title = title; Width = 720; Height = 460; MinWidth = 440; MinHeight = 300;
        WindowStartupLocation = WindowStartupLocation.CenterOwner;
        SetResourceReference(BackgroundProperty, "LayerFillColorDefaultBrush");
        SetResourceReference(ForegroundProperty, "TextFillColorPrimaryBrush");
        output.SetResourceReference(Control.BackgroundProperty, "CardBackgroundFillColorDefaultBrush");
        output.SetResourceReference(Control.ForegroundProperty, "TextFillColorPrimaryBrush");
        var layout = new Grid { Margin = new Thickness(20) };
        layout.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        layout.RowDefinitions.Add(new RowDefinition());
        layout.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        var heading = new DockPanel { Margin = new Thickness(0, 0, 0, 16) };
        ring.Margin = new Thickness(0, 0, 12, 0); heading.Children.Add(ring);
        status.Text = title; status.FontWeight = FontWeights.SemiBold; heading.Children.Add(status);
        layout.Children.Add(heading);
        var card = new Border { Style = (Style)Application.Current.Resources["Card"], Child = output, Padding = new Thickness(12) };
        Grid.SetRow(card, 1); layout.Children.Add(card);
        var actions = new StackPanel { Orientation = Orientation.Horizontal, HorizontalAlignment = HorizontalAlignment.Right, Margin = new Thickness(0, 16, 0, 0) };
        var copy = new Button { Content = Localized.Get("复制输出"), Margin = new Thickness(0, 0, 8, 0) };
        copy.Click += (_, _) =>
        {
            try { if (!string.IsNullOrEmpty(output.Text)) Clipboard.SetText(output.Text); }
            catch (System.Runtime.InteropServices.COMException) { status.Text = Localized.Get("剪贴板暂不可用，请重试。"); }
        };
        cancel = new Button { Content = Localized.Get("取消操作"), Margin = new Thickness(0, 0, 8, 0) };
        cancel.Click += (_, _) => { cancel.IsEnabled = false; onCancel(); };
        var close = new Button { Content = Localized.Get("关闭") };
        close.Click += (_, _) => Hide();
        actions.Children.Add(copy); actions.Children.Add(cancel); actions.Children.Add(close);
        Grid.SetRow(actions, 2); layout.Children.Add(actions); Content = layout;
        Closing += OnClosing;
    }

    internal void Report(string message)
    {
        if (!IsRunning) return;
        status.Text = message;
        output.AppendText(message + Environment.NewLine);
        if (output.Text.Length > OutputLimit) output.Text = output.Text[^OutputLimit..];
        output.ScrollToEnd();
    }

    internal void Finish(string message, bool failed = false)
    {
        IsRunning = false; ring.IsActive = false; ring.Visibility = Visibility.Collapsed;
        cancel.Visibility = Visibility.Collapsed;
        status.Text = failed ? Localized.Get("操作失败") : Localized.Get("操作完成。");
        output.Text = message.Length > OutputLimit ? message[..OutputLimit] + "\n…" : message;
        output.ScrollToHome();
    }

    internal void Dismiss() { dismissing = true; Close(); }
    private void OnClosing(object? sender, CancelEventArgs args)
    {
        if (dismissing || Application.Current.Dispatcher.HasShutdownStarted) return;
        args.Cancel = true; Hide();
    }
}
