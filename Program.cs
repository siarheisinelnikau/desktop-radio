using System.Diagnostics;
using System.Text.Json;

namespace DesktopRadio;

internal static class Program
{
    [STAThread]
    private static int Main(string[] args)
    {
        if (args is ["--apply-update", ..]) return Updates.Apply(args);
        ApplicationConfiguration.Initialize();
        if (args is ["--capture", var path, var page] && page is "radio" or "equalizer" or "settings" or "stations")
        {
            using var capture = new RadioWindow(new PreferencesStore(Path.GetTempPath()), diagnostic: true);
            capture.CaptureScreen(path, page);
            return 0;
        }
        if (args is ["--diagnose-ui", var report]) return DiagnoseUi(report);
        if (args is ["--diagnose-audio", var source, var result]) return DiagnoseAudio(source, result);
        if (args.Length != 0 && (args.Length != 2 || args[0] is not ("--cleanup-updater" or "--update-failed"))) return 2;

        using var activation = new EventWaitHandle(false, EventResetMode.AutoReset,
            @"Local\DesktopRadio.Activate", out var firstInstance);
        if (!firstInstance) { activation.Set(); return 0; }
        using var context = new ApplicationContext();
        RadioWindow window = null!;
        var quitting = false;
        async void Quit()
        {
            if (quitting) return;
            quitting = true;
            await window.Shutdown();
            context.ExitThread();
        }

        using (window = new(new PreferencesStore(PreferencesStore.DefaultDirectory), diagnostic: false, Quit))
        {
            var wait = ThreadPool.RegisterWaitForSingleObject(activation, (_, _) =>
            {
                try { window.BeginInvoke(() => { if (!quitting) window.ShowPanel(); }); }
                catch (InvalidOperationException) { }
            }, null, Timeout.Infinite, false);
            try
            {
                if (args.Length == 2)
                {
                    _ = Updates.Cleanup(args[1]);
                    if (args[0] == "--update-failed") window.ShowUpdateError();
                }
                window.Start();
                Application.Run(context);
            }
            finally { wait.Unregister(null); }
        }
        return 0;
    }

    private static int DiagnoseAudio(string source, string report)
    {
        var events = new System.Collections.Concurrent.ConcurrentQueue<Playback>();
        using var player = new RadioPlayer();
        player.SetMixer(new Preferences { Volume = 0, EqualizerEnabled = true, Equalizer = [0, 0, 0, 0, 0, 6, 0, 0, 0, 0] });
        player.SetVisualization(true);
        player.Start(new Station("fixture", "Fixture", "", "", [source], "BELARUS", null, null, null, 0), events.Enqueue);
        var completed = player.Completion.Wait(TimeSpan.FromSeconds(26));
        player.Stop();

        var input = new short[96000];
        var filtered = new short[input.Length];
        for (var i = 0; i < input.Length; i += 2)
            input[i] = input[i + 1] = (short)(1000 * Math.Sin((i / 2) * 2 * Math.PI * 1000 / EqualizerProcessor.SampleRate));
        var equalizer = new EqualizerProcessor();
        equalizer.Configure([0, 0, 0, 0, 0, 6, 0, 0, 0, 0]);
        equalizer.Process(input, filtered, 1);
        var response = filtered.Skip(24000).Max() / (double)input.Max();
        File.WriteAllText(report, JsonSerializer.Serialize(new { completed, audioBlocks = player.Meter.Revision,
            events = events.Select(item => item.State.ToString()), equalizerGainAt1Khz = response }, Catalog.Json));
        return completed && player.Meter.Revision > 0 && response is > 1.9 and < 2.1 ? 0 : 1;
    }

    private static int DiagnoseUi(string report)
    {
        using var window = new RadioWindow(new PreferencesStore(Path.GetTempPath()), diagnostic: true);
        using var context = new ApplicationContext();
        using var timer = new System.Windows.Forms.Timer();
        using var process = Process.GetCurrentProcess();
        var painted = new PaintMessages();
        Application.AddMessageFilter(painted);
        var time = Stopwatch.StartNew();
        var data = new List<object>();
        void Snapshot(string state)
        {
            process.Refresh();
            data.Add(new { state, ms = time.ElapsedMilliseconds, cpuMs = process.TotalProcessorTime.TotalMilliseconds,
                formPaints = window.PaintCount, queuedPaints = painted.Count, dpi = window.DeviceDpi });
        }
        var phase = 0;
        timer.Tick += (_, _) =>
        {
            switch (phase++)
            {
                case 0: Snapshot("visible settled start"); timer.Interval = 7000; break;
                case 1: Snapshot("visible settled end"); window.Hide(); timer.Interval = 500; break;
                case 2: Snapshot("hidden settled start"); timer.Interval = 7000; break;
                case 3: Snapshot("hidden settled end"); window.ShowPanel(intro: false); timer.Interval = 900; break;
                default:
                    Snapshot("reopened");
                    timer.Stop();
                    File.WriteAllText(report, JsonSerializer.Serialize(new { pid = Environment.ProcessId, data }, Catalog.Json));
                    context.ExitThread();
                    break;
            }
        };
        window.ShowPanel(intro: false);
        timer.Interval = 900;
        timer.Start();
        Application.Run(context);
        Application.RemoveMessageFilter(painted);
        return 0;
    }

    private sealed class PaintMessages : IMessageFilter
    {
        public int Count { get; private set; }
        public bool PreFilterMessage(ref Message message)
        {
            if (message.Msg == 15) Count++;
            return false;
        }
    }
}
