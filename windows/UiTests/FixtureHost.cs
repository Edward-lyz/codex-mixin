using System.Net;
using System.Net.Sockets;
using System.Text;
using System.Text.Json;

namespace CodexMixin.UiTests;

// A loopback upstream fixture. The real Rust gateway records every demo request.
internal sealed class FixtureHost : IDisposable
{
    private readonly HttpListener listener = new();
    private readonly CancellationTokenSource lifetime = new();
    private readonly Task server;
    internal string Url { get; }

    internal FixtureHost()
    {
        Url = $"http://127.0.0.1:{FreePort()}";
        listener.Prefixes.Add($"{Url}/");
        listener.Start();
        server = ServeAsync();
    }

    internal static int FreePort()
    {
        var reservation = new TcpListener(IPAddress.Loopback, 0);
        try
        {
            reservation.Start();
            return ((IPEndPoint)reservation.LocalEndpoint).Port;
        }
        finally { reservation.Stop(); }
    }

    private async Task ServeAsync()
    {
        try
        {
            while (!lifetime.IsCancellationRequested)
            {
                var context = await listener.GetContextAsync().WaitAsync(lifetime.Token);
                string response;
                if (context.Request.HttpMethod == "GET" && context.Request.Url?.AbsolutePath == "/quota")
                    response = JsonSerializer.Serialize(new { used = 81, limit = 100, remaining = 19 });
                else if (context.Request.HttpMethod == "POST" && context.Request.Url?.AbsolutePath == "/v1/chat/completions")
                {
                    using var request = await JsonDocument.ParseAsync(context.Request.InputStream, cancellationToken: lifetime.Token);
                    var model = request.RootElement.GetProperty("model").GetString();
                    response = JsonSerializer.Serialize(new
                    {
                        id = "ci-reply", @object = "chat.completion", created = 1, model,
                        choices = new[] { new { index = 0, message = new { role = "assistant", content = "CI fixture reply" }, finish_reason = "stop" } },
                        usage = new { prompt_tokens = model == "ci-coder" ? 1000 : 500, completion_tokens = 32, total_tokens = model == "ci-coder" ? 1032 : 532 }
                    });
                }
                else
                {
                    context.Response.StatusCode = 404;
                    context.Response.Close();
                    throw new InvalidOperationException($"Unexpected upstream fixture request: {context.Request.HttpMethod} {context.Request.Url?.AbsolutePath}");
                }
                var bytes = Encoding.UTF8.GetBytes(response);
                context.Response.ContentType = "application/json";
                context.Response.ContentLength64 = bytes.Length;
                await context.Response.OutputStream.WriteAsync(bytes, lifetime.Token);
                context.Response.Close();
            }
        }
        catch (OperationCanceledException) when (lifetime.IsCancellationRequested) { }
        catch (HttpListenerException) when (lifetime.IsCancellationRequested) { }
        catch (ObjectDisposedException) when (lifetime.IsCancellationRequested) { }
    }

    public void Dispose()
    {
        lifetime.Cancel();
        listener.Close();
        server.GetAwaiter().GetResult();
        lifetime.Dispose();
    }
}
