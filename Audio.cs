using System.Diagnostics;
using System.Net.Http.Json;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Text.Json;
using Microsoft.Win32;

namespace DesktopRadio;

internal enum PlayState { Ready, Connecting, Playing, Unavailable, AudioError, EngineMissing }
internal sealed record Playback(PlayState State, string? Track = null);
internal sealed record Mixer(int Volume, bool Enabled, int[] Gains);

internal sealed class RadioPlayer : IDisposable
{
    private static readonly HttpClient MetadataClient = new() { Timeout = TimeSpan.FromSeconds(4) };
    private CancellationTokenSource? cancellation;
    private Task worker = Task.CompletedTask;
    private Mixer mixer = new(65, false, []);
    private bool visualize;
    public AudioMeter Meter { get; } = new();

    public void SetMixer(Preferences preferences) => Volatile.Write(ref mixer,
        new(Math.Clamp(preferences.Volume, 0, 100), preferences.EqualizerEnabled, (int[])preferences.Equalizer.Clone()));

    public void SetVisualization(bool enabled) => Volatile.Write(ref visualize, enabled);

    public void Start(Station station, Action<Playback> report)
    {
        Stop();
        var previous = worker;
        var source = new CancellationTokenSource();
        cancellation = source;
        worker = Task.Run(async () =>
        {
            try
            {
                await previous.ConfigureAwait(false);
                if (!source.IsCancellationRequested)
                    await Play(station, report, source.Token).ConfigureAwait(false);
            }
            finally
            {
                source.Dispose();
            }
        });
    }

    public void Stop()
    {
        try { cancellation?.Cancel(); }
        catch (ObjectDisposedException) { }
        cancellation = null;
    }

    public Task Completion => worker;
    public void Dispose() => Stop();

    private async Task Play(Station station, Action<Playback> report, CancellationToken token)
    {
        try
        {
            using var engine = new VlcEngine();
            using var output = new WaveOutput();
            foreach (var stream in station.Streams)
            {
                token.ThrowIfCancellationRequested();
                report(new(PlayState.Connecting));
                if (await PlayStream(engine, output, stream, station.TrackSourceId, report, token).ConfigureAwait(false))
                    return;
            }
            if (!token.IsCancellationRequested) report(new(PlayState.Unavailable));
        }
        catch (OperationCanceledException) when (token.IsCancellationRequested) { }
        catch (Exception exception) when (exception is DllNotFoundException or BadImageFormatException
           or EntryPointNotFoundException or TypeInitializationException)
        {
            if (!token.IsCancellationRequested) report(new(PlayState.EngineMissing));
        }
        catch (Exception)
        {
            if (!token.IsCancellationRequested) report(new(PlayState.AudioError));
        }
    }

    private async Task<bool> PlayStream(VlcEngine engine, WaveOutput output, string stream, int trackSourceId,
        Action<Playback> report, CancellationToken token)
    {
        using var player = engine.Open(stream);
        var equalizer = new EqualizerProcessor();
        Mixer? current = null;
        var volume = 0f;
        var lastAudio = 0L;
        var audioFailed = 0;
        Vlc.PlayAudio play = (_, samples, frames, _) =>
        {
            try
            {
                var next = Volatile.Read(ref mixer);
                if (!ReferenceEquals(current, next))
                {
                    if (current is null || current.Enabled != next.Enabled || !current.Gains.SequenceEqual(next.Gains))
                        equalizer.Configure(next.Enabled ? next.Gains : []);
                    current = next;
                    volume = next.Volume / 100f;
                }
                output.Write(samples, frames, equalizer, volume, Volatile.Read(ref visualize) ? Meter : null, token);
                Volatile.Write(ref lastAudio, Stopwatch.GetTimestamp());
            }
            catch (Exception) { Volatile.Write(ref audioFailed, 1); }
        };
        Vlc.FlushAudio flush = (_, _) => output.Flush();
        player.InstallAudio(play, flush);
        if (!player.Start()) return false;
        var started = Stopwatch.GetTimestamp();
        var nextMetadata = DateTimeOffset.MinValue;
        Task<string?>? metadata = null;
        string? feedTrack = null;
        Playback? lastPublished = null;
        while (!token.IsCancellationRequested)
        {
            var last = Volatile.Read(ref lastAudio);
            if (Volatile.Read(ref audioFailed) != 0) throw new InvalidOperationException("Audio output failed.");
            var state = player.State;
            if (state == 6 && new Uri(stream).IsFile && last != 0) return true;
            if (state is 5 or 6 or 7 || Stopwatch.GetElapsedTime(last == 0 ? started : last) > TimeSpan.FromSeconds(12))
                return false;
            if (last != 0)
            {
                var track = player.NowPlaying;
                if (metadata?.IsCompletedSuccessfully == true)
                {
                    feedTrack = metadata.Result;
                    metadata = null;
                }
                if (trackSourceId > 0 && string.IsNullOrWhiteSpace(track) && metadata is null &&
                    DateTimeOffset.UtcNow >= nextMetadata)
                {
                    metadata = ReadTrack(trackSourceId, token);
                    nextMetadata = DateTimeOffset.UtcNow.AddSeconds(18);
                }
                var playback = new Playback(PlayState.Playing, string.IsNullOrWhiteSpace(track) ? feedTrack : track);
                if (lastPublished != playback) report(lastPublished = playback);
            }
            await Task.Delay(last == 0 ? 100 : 500, token).ConfigureAwait(false);
        }
        return true;
    }

    private static async Task<string?> ReadTrack(int stationId, CancellationToken token)
    {
        try
        {
            using var document = await MetadataClient.GetFromJsonAsync<JsonDocument>(
                $"https://radioplayer.by/api/songs/current/{stationId}", token).ConfigureAwait(false);
            if (document is null) return null;
            var track = document.RootElement;
            if (!track.GetProperty("hasData").GetBoolean() || track.GetProperty("id").GetInt32() <= 0) return null;
            var start = track.GetProperty("lastStartPlaying").GetDateTimeOffset();
            var duration = track.GetProperty("duration").GetInt32();
            var now = DateTimeOffset.UtcNow;
            if (duration is <= 0 or > 21600 || start > now.AddSeconds(30) || start.AddSeconds(duration + 25) < now)
                return null;
            var title = track.GetProperty("name").GetString();
            var artist = track.TryGetProperty("artist", out var value) ? value.GetString() : null;
            return string.IsNullOrWhiteSpace(artist) ? title : $"{artist} — {title}";
        }
        catch (Exception) { return null; }
    }
}

internal sealed class AudioMeter
{
    public const int SampleCount = 2048;
    private readonly float[] samples = new float[SampleCount];
    private int next;
    private int revision;
    public int Revision => Volatile.Read(ref revision);

    public void Write(ReadOnlySpan<short> audio)
    {
        var from = Math.Max(0, audio.Length - SampleCount * 2);
        lock (samples)
        {
            for (var i = from; i < audio.Length; i += 2)
            {
                samples[next] = (audio[i] + audio[i + 1]) / 65536f;
                next = (next + 1) % SampleCount;
            }
            Interlocked.Increment(ref revision);
        }
    }

    public int CopyTo(float[] other)
    {
        lock (samples)
        {
            Array.Copy(samples, next, other, 0, samples.Length - next);
            Array.Copy(samples, 0, other, samples.Length - next, next);
            return revision;
        }
    }
}

internal sealed class EqualizerProcessor
{
    public const int SampleRate = 48000;
    public static readonly int[] Frequencies = [31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];
    private readonly Filter[] filters = new Filter[Frequencies.Length];
    private int count;

    public void Configure(int[] gains)
    {
        count = 0;
        for (var band = 0; band < gains.Length; band++)
        {
            var gain = Math.Clamp(gains[band], -12, 12);
            if (gain == 0) continue;
            var omega = 2 * Math.PI * Frequencies[band] / SampleRate;
            var amplitude = Math.Pow(10, gain / 40.0);
            var alpha = Math.Sin(omega) / (2 * 1.41);
            var middle = -2 * Math.Cos(omega);
            var denominator = 1 + alpha / amplitude;
            filters[count++] = new((float)((1 + alpha * amplitude) / denominator), (float)(middle / denominator),
                (float)((1 - alpha * amplitude) / denominator), (float)((1 - alpha / amplitude) / denominator));
        }
    }

    public void Process(ReadOnlySpan<short> input, Span<short> output, float volume)
    {
        if (count == 0 && volume == 1f)
        {
            input.CopyTo(output);
            return;
        }
        for (var i = 0; i < input.Length; i += 2)
        {
            float left = input[i], right = input[i + 1];
            for (var band = 0; band < count; band++)
                filters[band].Process(ref left, ref right);
            output[i] = (short)Math.Clamp(left * volume, short.MinValue, short.MaxValue);
            output[i + 1] = (short)Math.Clamp(right * volume, short.MinValue, short.MaxValue);
        }
    }

    private struct Filter(float b0, float b1, float b2, float a2)
    {
        private float l1, l2, r1, r2;

        public void Process(ref float left, ref float right)
        {
            var l = b0 * left + l1;
            var r = b0 * right + r1;
            l1 = b1 * (left - l) + l2;
            r1 = b1 * (right - r) + r2;
            l2 = b2 * left - a2 * l;
            r2 = b2 * right - a2 * r;
            left = l;
            right = r;
        }
    }
}

internal sealed unsafe class WaveOutput : IDisposable
{
    private const int BufferBytes = 19200;
    private const uint Done = 1;
    private const uint InQueue = 16;
    private static readonly uint HeaderSize = (uint)sizeof(Header);
    private readonly nint[] buffers = new nint[8];
    private readonly EventWaitHandle completed = new(false, EventResetMode.AutoReset);
    private nint device;
    private int next;

    public WaveOutput()
    {
        var format = new Format { Type = 1, Channels = 2, Rate = EqualizerProcessor.SampleRate, BytesPerSecond = 192000,
            BlockAlign = 4, Bits = 16 };
        try
        {
            Check(waveOutOpen(out device, uint.MaxValue, ref format, completed.SafeWaitHandle.DangerousGetHandle(), 0, 0x50000));
            for (var i = 0; i < buffers.Length; i++)
            {
                var header = (Header*)NativeMemory.AllocZeroed(HeaderSize + BufferBytes);
                buffers[i] = (nint)header;
                header->Data = (nint)(header + 1);
                header->Length = BufferBytes;
                Check(waveOutPrepareHeader(device, header, HeaderSize));
            }
        }
        catch
        {
            Dispose();
            throw;
        }
    }

    public void Write(nint samples, uint frames, EqualizerProcessor processor, float volume, AudioMeter? meter,
        CancellationToken token)
    {
        var source = new ReadOnlySpan<short>((void*)samples, checked((int)frames * 2));
        while (!source.IsEmpty && !token.IsCancellationRequested)
        {
            var header = (Header*)buffers[next];
            var deadline = Stopwatch.GetTimestamp();
            while ((Volatile.Read(ref header->Flags) & (InQueue | Done)) == InQueue)
            {
                if (token.IsCancellationRequested) return;
                if (Stopwatch.GetElapsedTime(deadline) > TimeSpan.FromSeconds(1))
                    throw new IOException("The audio device stopped accepting samples.");
                completed.WaitOne(15);
            }
            var count = Math.Min(BufferBytes / sizeof(short), source.Length);
            var destination = new Span<short>((void*)header->Data, count);
            processor.Process(source[..count], destination, volume);
            meter?.Write(destination);
            header->Length = (uint)(count * sizeof(short));
            Check(waveOutWrite(device, header, HeaderSize));
            source = source[count..];
            next = (next + 1) % buffers.Length;
        }
    }

    public void Flush()
    {
        if (device != 0) waveOutReset(device);
    }

    public void Dispose()
    {
        Flush();
        foreach (var pointer in buffers)
        {
            if (pointer == 0) continue;
            var header = (Header*)pointer;
            if ((header->Flags & 2) != 0) waveOutUnprepareHeader(device, header, HeaderSize);
            NativeMemory.Free(header);
        }
        if (device != 0) waveOutClose(device);
        device = 0;
        completed.Dispose();
    }

    private static void Check(uint error)
    {
        if (error != 0) throw new IOException($"Windows audio output failed ({error}).");
    }

    [StructLayout(LayoutKind.Sequential, Pack = 2)]
    private struct Format { public ushort Type, Channels; public uint Rate, BytesPerSecond; public ushort BlockAlign, Bits, Extra; }

    [StructLayout(LayoutKind.Sequential)]
    private struct Header { public nint Data; public uint Length, Recorded; public nuint User; public uint Flags, Loops; public nint Next, Reserved; }

    [DllImport("winmm.dll")] private static extern uint waveOutOpen(out nint device, uint index, ref Format format, nint callback, nint instance, uint flags);
    [DllImport("winmm.dll")] private static extern uint waveOutPrepareHeader(nint device, Header* header, uint size);
    [DllImport("winmm.dll")] private static extern uint waveOutUnprepareHeader(nint device, Header* header, uint size);
    [DllImport("winmm.dll")] private static extern uint waveOutWrite(nint device, Header* header, uint size);
    [DllImport("winmm.dll")] private static extern uint waveOutReset(nint device);
    [DllImport("winmm.dll")] private static extern uint waveOutClose(nint device);
}

internal sealed class VlcEngine : IDisposable
{
    private readonly nint engine;
    public VlcEngine()
    {
        Vlc.EnsureLoaded();
        engine = Vlc.libvlc_new(0, nint.Zero);
        if (engine == 0) throw new DllNotFoundException("VLC could not be initialized.");
    }
    public VlcStream Open(string address) => new(engine, address);
    public void Dispose() => Vlc.libvlc_release(engine);
}

internal sealed class VlcStream : IDisposable
{
    private readonly nint media;
    private readonly nint player;
    private Vlc.PlayAudio? play;
    private Vlc.FlushAudio? flush;
    public VlcStream(nint engine, string address)
    {
        media = Vlc.libvlc_media_new_location(engine, address);
        if (media == 0) throw new IOException("Could not open stream.");
        Vlc.libvlc_media_add_option(media, ":network-caching=1200");
        Vlc.libvlc_media_add_option(media, ":no-video");
        player = Vlc.libvlc_media_player_new_from_media(media);
        if (player != 0) return;
        Vlc.libvlc_media_release(media);
        throw new IOException("Could not create a VLC media player.");
    }

    public void InstallAudio(Vlc.PlayAudio onPlay, Vlc.FlushAudio onFlush)
    {
        play = onPlay;
        flush = onFlush;
        Vlc.libvlc_audio_set_callbacks(player, play, 0, 0, flush, 0, 0);
        Vlc.libvlc_audio_set_format(player, "S16N", EqualizerProcessor.SampleRate, 2);
    }
    public bool Start() => Vlc.libvlc_media_player_play(player) == 0;
    public int State => Vlc.libvlc_media_player_get_state(player);
    public string? NowPlaying
    {
        get
        {
            var value = Vlc.libvlc_media_get_meta(media, 12);
            if (value == 0) return null;
            try { return Marshal.PtrToStringUTF8(value); }
            finally { Vlc.libvlc_free(value); }
        }
    }

    public void Dispose()
    {
        Vlc.libvlc_media_player_stop(player);
        Vlc.libvlc_media_player_release(player);
        Vlc.libvlc_media_release(media);
        GC.KeepAlive(play);
        GC.KeepAlive(flush);
    }
}

internal static class Vlc
{
    private const string Library = "RadioLibVlc";

    static Vlc() => NativeLibrary.SetDllImportResolver(typeof(Vlc).Assembly, Resolve);

    public static void EnsureLoaded()
    {
        var version = Marshal.PtrToStringUTF8(libvlc_get_version());
        if (version is null || !version.StartsWith("3.", StringComparison.Ordinal))
            throw new DllNotFoundException("VLC 3 is required.");
    }

    private static nint Resolve(string libraryName, Assembly assembly, DllImportSearchPath? searchPath)
    {
        if (libraryName != Library) return 0;
        using var registry = Registry.LocalMachine.OpenSubKey(@"SOFTWARE\VideoLAN\VLC");
        var installed = registry?.GetValue("InstallDir") as string;
        if (string.IsNullOrWhiteSpace(installed) && registry?.GetValue(null) is string executable)
            installed = Path.GetDirectoryName(executable);
        var fallback = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), "VideoLAN", "VLC");
        foreach (var directory in new[] { Path.Combine(AppContext.BaseDirectory, "vlc"), installed, fallback })
        {
            if (string.IsNullOrWhiteSpace(directory) || !Path.IsPathFullyQualified(directory)) continue;
            var library = Path.Combine(directory, "libvlc.dll");
            if (File.Exists(library) && NativeLibrary.TryLoad(library, assembly,
                    DllImportSearchPath.UseDllDirectoryForDependencies | DllImportSearchPath.SafeDirectories, out var handle))
                return handle;
        }
        throw new DllNotFoundException("VLC 3 (64-bit) was not found.");
    }

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] internal delegate void PlayAudio(nint data, nint samples, uint frames, long pts);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] internal delegate void FlushAudio(nint data, long pts);
    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)] internal static extern nint libvlc_get_version();
    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)] internal static extern nint libvlc_new(int count, nint arguments);
    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)] internal static extern void libvlc_release(nint instance);
    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)] internal static extern nint libvlc_media_new_location(nint instance, [MarshalAs(UnmanagedType.LPUTF8Str)] string location);
    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)] internal static extern void libvlc_media_add_option(nint media, [MarshalAs(UnmanagedType.LPUTF8Str)] string option);
    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)] internal static extern nint libvlc_media_get_meta(nint media, int key);
    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)] internal static extern void libvlc_media_release(nint media);
    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)] internal static extern void libvlc_free(nint value);
    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)] internal static extern nint libvlc_media_player_new_from_media(nint media);
    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)] internal static extern int libvlc_media_player_play(nint player);
    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)] internal static extern int libvlc_media_player_get_state(nint player);
    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)] internal static extern void libvlc_media_player_stop(nint player);
    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)] internal static extern void libvlc_media_player_release(nint player);
    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)] internal static extern void libvlc_audio_set_callbacks(nint player, PlayAudio play, nint pause, nint resume, FlushAudio flush, nint drain, nint data);
    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)] internal static extern void libvlc_audio_set_format(nint player, [MarshalAs(UnmanagedType.LPStr)] string format, uint rate, uint channels);
}
