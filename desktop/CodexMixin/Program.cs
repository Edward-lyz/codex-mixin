using Avalonia;
using TKCode.Arc;
using System.IO.Pipes;
using System.Security.Cryptography;
using System.Text;
using Avalonia.Threading;

namespace CodexMixin;

internal static class Program
{
    private static readonly string InstanceName = "CodexMixin.UI." + Convert.ToHexString(
        SHA256.HashData(Encoding.UTF8.GetBytes(Environment.UserName + "|" + AppContext.BaseDirectory)))[..20];

    [STAThread]
    public static void Main(string[] args)
    {
        using var instance = new Mutex(false, InstanceName);
        bool primary;
        try { primary = instance.WaitOne(0); }
        catch (AbandonedMutexException) { primary = true; }
        if (!primary)
        {
            try
            {
                using var client = new NamedPipeClientStream(".", InstanceName, PipeDirection.Out, PipeOptions.CurrentUserOnly);
                client.Connect(5000);
                var page = args.FirstOrDefault(arg => arg.StartsWith("--page=", StringComparison.Ordinal))?[7..] ?? "providers";
                client.Write(Encoding.UTF8.GetBytes(page));
                client.Flush();
            }
            catch (Exception error) when (error is IOException or TimeoutException)
            { Console.Error.WriteLine(error.Message); Environment.ExitCode = 1; }
            return;
        }
        try
        {
            AppBuilder.Configure<App>().UsePlatformDetect()
                .UseArc(arc => arc.UseArcBlocks().UseArcWorkflow().UseArcAdvancedCharts()
                    .WithTheme(theme => theme.Appearance(ArcAppearance.System).Accent(ArcAccentPreset.Neutral)))
                .LogToTrace().StartWithClassicDesktopLifetime(args);
        }
        finally { instance.ReleaseMutex(); }
    }

    internal static async Task ListenAsync(MainWindow window, CancellationToken token)
    {
        try
        {
            while (!token.IsCancellationRequested)
            {
                using var pipe = new NamedPipeServerStream(InstanceName, PipeDirection.In, 1,
                    PipeTransmissionMode.Byte, PipeOptions.Asynchronous | PipeOptions.CurrentUserOnly);
                await pipe.WaitForConnectionAsync(token).ConfigureAwait(false);
                try
                {
                    using var deadline = CancellationTokenSource.CreateLinkedTokenSource(token);
                    deadline.CancelAfter(TimeSpan.FromSeconds(5));
                    var buffer = new byte[64];
                    var count = 0;
                    while (count < buffer.Length)
                    {
                        var read = await pipe.ReadAsync(buffer.AsMemory(count), deadline.Token).ConfigureAwait(false);
                        if (read == 0) break;
                        count += read;
                    }
                    var page = Encoding.UTF8.GetString(buffer, 0, count);
                    if (page is "providers" or "agents" or "routing" or "usage" or "context" or "settings")
                        await Dispatcher.UIThread.InvokeAsync(() => window.OpenPage(page));
                }
                catch (OperationCanceledException) when (!token.IsCancellationRequested) { }
                catch (IOException) { /* A disconnected caller must not stop later activations. */ }
            }
        }
        catch (OperationCanceledException) when (token.IsCancellationRequested) { }
        catch (Exception error) when (error is IOException or OperationCanceledException)
        { await Diagnostics.RecordAsync("shell", "instance_listener_failed type=" + error.GetType().Name); }
    }
}
