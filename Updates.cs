using System.Diagnostics;
using System.Net;
using System.Net.Http.Json;
using System.Reflection;
using System.Security.Cryptography;
using System.Text.Json.Serialization;

namespace DesktopRadio;

internal sealed record UpdateAsset(string Name, long Size,
    [property: JsonPropertyName("browser_download_url")] Uri BrowserDownloadUrl);
internal sealed record GitHubRelease([property: JsonPropertyName("tag_name")] string TagName, bool Draft,
    bool Prerelease, UpdateAsset[] Assets);
internal sealed record WindowsUpdate(Version Version, UpdateAsset Executable, UpdateAsset Checksum);

internal static class Updates
{
    private const string Repository = "siarheisinelnikau/desktop-radio";
    private static readonly HttpClient Client = new() { Timeout = TimeSpan.FromSeconds(15) };
    private static string DirectoryPath => Path.Combine(PreferencesStore.DefaultDirectory, "Updates");
    public static Version Current { get; } = Version.Parse(typeof(Updates).Assembly
        .GetCustomAttribute<AssemblyInformationalVersionAttribute>()!.InformationalVersion.Split('+')[0]);
    public static bool IsPortable => string.IsNullOrEmpty(typeof(Updates).Assembly.Location);

    static Updates() => Client.DefaultRequestHeaders.UserAgent.ParseAdd($"DesktopRadio/{Current}");

    public static async Task<(WindowsUpdate? Update, string Message)> Check(CancellationToken token)
    {
        using var response = await Client.GetAsync($"https://api.github.com/repos/{Repository}/releases/latest", token);
        if (response.StatusCode == HttpStatusCode.NotFound)
            return (null, "No published Windows build is accessible.");
        response.EnsureSuccessStatusCode();
        var release = await response.Content.ReadFromJsonAsync<GitHubRelease>(cancellationToken: token);
        if (release is null || release.Draft || release.Prerelease ||
            !Version.TryParse(release.TagName.TrimStart('v'), out var version))
            return (null, "No published Windows build is accessible.");
        if (version <= Current) return (null, "You have the latest version.");
        var prefix = $"DesktopRadio-v{version}";
        var executable = release.Assets.FirstOrDefault(asset => asset.Name == $"{prefix}-x64.exe") ??
                         release.Assets.FirstOrDefault(asset => asset.Name == $"{prefix}.exe");
        var checksum = release.Assets.FirstOrDefault(asset => asset.Name == $"{executable?.Name}.sha256");
        if (executable is null || checksum is null || !IsOfficial(executable, release.TagName) ||
            !IsOfficial(checksum, release.TagName) || executable.Size is <= 0 or > 250_000_000 ||
            checksum.Size is <= 0 or > 1024)
            return (null, "No published Windows build is accessible.");
        return (new(version, executable, checksum), "Update available");
    }

    private static bool IsOfficial(UpdateAsset asset, string tag) => asset.BrowserDownloadUrl.IsAbsoluteUri &&
        asset.BrowserDownloadUrl.Scheme == Uri.UriSchemeHttps && asset.BrowserDownloadUrl.Host == "github.com" &&
        asset.BrowserDownloadUrl.AbsolutePath == $"/{Repository}/releases/download/{tag}/{asset.Name}";

    public static async Task Stage(WindowsUpdate update, IProgress<int> progress, CancellationToken token)
    {
        var installed = Environment.ProcessPath;
        if (!IsPortable || installed is null) throw new InvalidOperationException("This is not a portable build.");
        Directory.CreateDirectory(DirectoryPath);
        var session = Guid.NewGuid().ToString("N");
        var downloaded = Path.Combine(DirectoryPath, $"{session}.download");
        var helper = Path.Combine(DirectoryPath, $"{session}.exe");
        var started = false;
        using var timeout = CancellationTokenSource.CreateLinkedTokenSource(token);
        timeout.CancelAfter(TimeSpan.FromMinutes(3));
        try
        {
            using var checksumResponse = await Client.GetAsync(update.Checksum.BrowserDownloadUrl,
                HttpCompletionOption.ResponseHeadersRead, timeout.Token);
            checksumResponse.EnsureSuccessStatusCode();
            await using var checksumStream = await checksumResponse.Content.ReadAsStreamAsync(timeout.Token);
            var checksumBytes = new byte[1025];
            var checksumCount = await checksumStream.ReadAtLeastAsync(checksumBytes, checksumBytes.Length, false, timeout.Token);
            if (checksumCount > 1024) throw new InvalidDataException("The checksum is too large.");
            var expected = System.Text.Encoding.ASCII.GetString(checksumBytes, 0, checksumCount)
                .Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries).FirstOrDefault();
            if (expected is null || expected.Length != 64 || !expected.All(Uri.IsHexDigit))
                throw new InvalidDataException("Invalid release checksum.");

            using var response = await Client.GetAsync(update.Executable.BrowserDownloadUrl,
                HttpCompletionOption.ResponseHeadersRead, timeout.Token);
            response.EnsureSuccessStatusCode();
            using var hash = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
            await using (var source = await response.Content.ReadAsStreamAsync(timeout.Token))
            await using (var output = File.Create(downloaded))
            {
                var buffer = new byte[65536];
                var total = 0L;
                var lastPercent = -1;
                int count;
                while ((count = await source.ReadAsync(buffer, timeout.Token)) > 0)
                {
                    total += count;
                    if (total > update.Executable.Size) throw new InvalidDataException("Unexpected download size.");
                    hash.AppendData(buffer, 0, count);
                    await output.WriteAsync(buffer.AsMemory(0, count), timeout.Token);
                    var percent = (int)(total * 100 / update.Executable.Size);
                    if (percent != lastPercent) progress.Report(lastPercent = percent);
                }
                if (total != update.Executable.Size) throw new InvalidDataException("Incomplete download.");
            }
            if (!Convert.ToHexString(hash.GetHashAndReset()).Equals(expected, StringComparison.OrdinalIgnoreCase))
                throw new InvalidDataException("The download does not match the published checksum.");
            // Verify write access before starting the helper and closing the radio.
            var sibling = $"{installed}.{session}.pending";
            try
            {
                File.Copy(downloaded, sibling);
                File.Copy(installed, helper);
                using var process = Start(helper, "--apply-update", installed, sibling, $"{Environment.ProcessId}", expected);
                started = true;
            }
            finally { if (!started) Delete(sibling); }
        }
        finally
        {
            Delete(downloaded);
            if (!started) Delete(helper);
        }
    }

    public static int Apply(string[] args)
    {
        var helper = Environment.ProcessPath!;
        var session = Path.GetFileNameWithoutExtension(helper);
        if (args.Length != 5 || !Guid.TryParseExact(session, "N", out _) ||
            !string.Equals(Path.GetDirectoryName(helper), DirectoryPath, StringComparison.OrdinalIgnoreCase) ||
            !int.TryParse(args[3], out var pid) || !Path.IsPathFullyQualified(args[1]) ||
            !string.Equals(args[2], $"{args[1]}.{session}.pending", StringComparison.OrdinalIgnoreCase))
            return 1;
        var installed = args[1];
        var pending = args[2];
        var backup = $"{installed}.{session}.backup";
        var applied = false;
        try
        {
            try
            {
                using var old = Process.GetProcessById(pid);
                if (!old.WaitForExit(20000)) throw new IOException("The radio did not exit.");
            }
            catch (ArgumentException) { }
            using (var content = File.OpenRead(pending))
            {
                if (!Convert.ToHexString(SHA256.HashData(content)).Equals(args[4], StringComparison.OrdinalIgnoreCase))
                    throw new InvalidDataException("The staged update does not match the published checksum.");
            }
            File.Replace(pending, installed, backup);
            applied = true;
            using (Start(installed, "--cleanup-updater", session)) { }
            Delete(backup);
            return 0;
        }
        catch
        {
            try
            {
                if (applied) File.Replace(backup, installed, null);
                using var process = Start(installed, "--update-failed", session);
            }
            catch { MessageBox.Show("The update could not be installed.", "Desktop Radio", MessageBoxButtons.OK, MessageBoxIcon.Error); }
            return 1;
        }
        finally { Delete(pending); }
    }

    public static async Task Cleanup(string session)
    {
        if (!Guid.TryParseExact(session, "N", out _)) return;
        var helper = Path.Combine(DirectoryPath, $"{session}.exe");
        for (var attempt = 0; attempt < 20 && File.Exists(helper); attempt++)
        {
            Delete(helper);
            if (File.Exists(helper)) await Task.Delay(250).ConfigureAwait(false);
        }
    }

    private static Process Start(string path, params string[] args)
    {
        var process = new ProcessStartInfo(path) { UseShellExecute = false };
        foreach (var arg in args) process.ArgumentList.Add(arg);
        return Process.Start(process) ?? throw new IOException("The update process could not start.");
    }

    private static void Delete(string path)
    {
        try { File.Delete(path); }
        catch (IOException) { }
        catch (UnauthorizedAccessException) { }
    }
}
