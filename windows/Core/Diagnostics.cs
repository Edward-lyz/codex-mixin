using System.Diagnostics;

namespace CodexMixin;

internal static class Diagnostics
{
    internal static string DirectoryPath => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "CodexMixin", "logs");
    private static readonly SemaphoreSlim gate = new(1, 1);

    // Callers provide timings/classifications only, never argv or raw CLI output.
    internal static async Task RecordAsync(string category, string line)
    {
        await gate.WaitAsync().ConfigureAwait(false);
        try
        {
            await Task.Run(() =>
            {
                Directory.CreateDirectory(DirectoryPath);
                var path = Path.Combine(DirectoryPath, category + ".log");
                if (File.Exists(path) && new FileInfo(path).Length > 1024 * 1024)
                    File.Move(path, path + ".previous", overwrite: true);
                File.AppendAllText(path, line + Environment.NewLine);
            }).ConfigureAwait(false);
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException)
        {
            Trace.TraceWarning("Cannot write GUI diagnostic log: {0}", error.GetType().Name);
        }
        finally { gate.Release(); }
    }
}
