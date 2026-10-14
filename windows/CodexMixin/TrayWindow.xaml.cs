using System.Text.Json;
using Microsoft.UI;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;

namespace CodexMixin;

// XAML window roots are framework activation types.
public sealed partial class TrayWindow : Window
{
    private readonly CancellationTokenSource lifetime = new();
    private JsonElement[] providers = [];
    private string? providerId;
    private string days = "1";
    private bool busy;
    private bool settingState;
    private bool pinned;
    private bool exiting;

    public TrayWindow()
    {
        InitializeComponent();
        Panel.RequestedTheme = ((App)Microsoft.UI.Xaml.Application.Current).Theme;
        Title = "Codex Mixin · 主面板";
        AppWindow.IsShownInSwitchers = false;
        var presenter = (OverlappedPresenter)AppWindow.Presenter;
        presenter.SetBorderAndTitleBar(false, false);
        presenter.IsResizable = false;
        presenter.IsMinimizable = false;
        presenter.IsMaximizable = false;
        presenter.IsAlwaysOnTop = true;
        Activated += (_, args) => { if (!pinned && !busy && args.WindowActivationState == WindowActivationState.Deactivated) AppWindow.Hide(); };
        Closed += (_, _) => lifetime.Cancel();
        AppWindow.Closing += (_, args) => { if (!exiting) { args.Cancel = true; AppWindow.Hide(); } };
    }

    internal void Shutdown() { exiting = true; Close(); }
    internal void SetTheme(ElementTheme theme) => Panel.RequestedTheme = theme;

    internal async void ShowPanel()
    {
        pinned = Environment.GetCommandLineArgs().Contains("--capture-ui");
        AppWindow.MoveAndResize(NativeShell.PanelBounds());
        AppWindow.Show(); Activate();
        await ReloadAsync();
    }

    private async Task ReloadAsync()
    {
        if (busy) return;
        busy = true; GatewaySwitch.IsEnabled = false;
        try
        {
            var client = ((App)Microsoft.UI.Xaml.Application.Current).Client;
            var status = await client.QueryAsync(["service", "status", "--json"], lifetime.Token);
            ServiceTitle.Text = $"本地服务 · {Json.Gateway(status)}";
            Endpoint.Text = Json.OptionalText(status, "endpoint") ?? "请先配置服务商";
            var running = Json.OptionalText(status, "gateway") == "running";
            ServiceDot.Fill = new SolidColorBrush(running ? Colors.LimeGreen : Colors.Gray);
            settingState = true; GatewaySwitch.IsOn = running; settingState = false;
            providers = Json.Flag(status, "configured")
                ? Json.Rows(Json.Field(await client.QueryAsync(["provider", "list", "--json"], lifetime.Token), "providers")) : [];
            ProviderStrip.Children.Clear();
            foreach (var provider in providers)
            {
                var id = Json.Text(provider, "id");
                var button = new Button { Content = Marks.Provider(provider, 24), Padding = new Thickness(8) };
                ToolTipService.SetToolTip(button, Json.Text(provider, "display_name"));
                button.Click += async (_, _) => { providerId = id; await ReloadAsync(); };
                ProviderStrip.Children.Add(button);
            }
            providerId ??= providers.FirstOrDefault().ValueKind == JsonValueKind.Object ? Json.Text(providers[0], "id") : null;
            var selected = providers.FirstOrDefault(row => Json.Text(row, "id") == providerId);
            ProviderTitle.Text = selected.ValueKind == JsonValueKind.Object ? Json.Text(selected, "display_name") : "尚未配置服务商";
            UsageBody.Children.Clear(); Quota.Text = "额度未知";
            if (Json.Flag(status, "configured"))
            {
                var quotas = Json.Rows(await client.QueryAsync(["quota", "--json"], lifetime.Token));
                var row = quotas.FirstOrDefault(row => Json.OptionalText(row, "provider_id") == providerId && Json.OptionalText(row, "error") == null);
                if (row.ValueKind == JsonValueKind.Object) Quota.Text = $"余额 {Json.Field(row, "remaining")} {Json.OptionalText(row, "currency")}";
            }
            if (!running) UsageBody.Children.Add(new TextBlock { Text = "本地网关未运行，暂无可读取的用量。", TextWrapping = TextWrapping.Wrap, FontSize = 12 });
            else
            {
                var arguments = new List<string> { "usage", "--json" };
                if (days != "all") arguments.AddRange(["--days", days]);
                var rows = Json.Rows(await client.QueryAsync(arguments.ToArray(), lifetime.Token)).Where(row => Json.Text(row, "provider_id") == providerId).ToArray();
                foreach (var row in rows)
                {
                    var body = new StackPanel { Spacing = 8 };
                    body.Children.Add(new TextBlock { Text = Json.Text(row, "model_id"), FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, TextWrapping = TextWrapping.Wrap });
                    body.Children.Add(new TextBlock { Text = $"请求 {Json.Field(row, "request_count")} · 输入 {Json.Field(row, "input_tokens")}\n缓存输入 {Json.Field(row, "cache_read_tokens")} · 输出 {Json.Field(row, "output_tokens")}", TextWrapping = TextWrapping.Wrap, FontSize = 12 });
                    var output = Json.Field(row, "output_tokens").GetDouble();
                    body.Children.Add(new ProgressBar { Value = output, Maximum = Math.Max(1, rows.Max(value => Json.Field(value, "output_tokens").GetDouble())) });
                    UsageBody.Children.Add(new Border { Style = (Style)Microsoft.UI.Xaml.Application.Current.Resources["Card"], Child = body });
                }
                if (rows.Length == 0) UsageBody.Children.Add(new TextBlock { Text = "此时间范围暂无请求记录。" });
            }
            PanelStatus.Text = $"{DateTime.Now:HH:mm:ss} · 配置已同步";
        }
        catch (Exception error)
        {
            ServiceTitle.Text = "本地服务 · 状态未知";
            ServiceDot.Fill = new SolidColorBrush(Colors.Orange);
            PanelStatus.Text = error.Message;
        }
        finally { busy = false; GatewaySwitch.IsEnabled = true; }
    }

    private async void OnGatewayToggled(object sender, RoutedEventArgs args)
    {
        if (settingState || busy) return;
        await ChangeAsync(["service", GatewaySwitch.IsOn ? "start" : "stop", "--managed", "--json"]);
    }
    private async void OnRange(object sender, RoutedEventArgs args) { days = (string)((Button)sender).Tag; await ReloadAsync(); }
    private void OnOpen(object sender, RoutedEventArgs args)
    {
        var target = (string)((FrameworkElement)sender).Tag;
        AppWindow.Hide(); ((App)Microsoft.UI.Xaml.Application.Current).OpenPage(target);
    }
    private async void OnAutostart(object sender, RoutedEventArgs args) => await ChangeAsync(["service", "autostart", "enable", "--json"]);
    private async void OnDoctor(object sender, RoutedEventArgs args) => await ChangeAsync(["doctor", "--quick", "--json"]);
    private async Task ChangeAsync(string[] args)
    {
        if (busy) return;
        busy = true;
        try { PanelStatus.Text = await ((App)Microsoft.UI.Xaml.Application.Current).Operations.ChangeAsync(args, lifetime.Token); }
        catch (Exception error) { PanelStatus.Text = error.Message; }
        finally { busy = false; }
        await ReloadAsync();
    }
}
