using System.ComponentModel;
using System.Drawing.Drawing2D;
using System.Numerics;

namespace DesktopRadio;

internal sealed class Visualization(AudioMeter meter) : Control
{
    private readonly float[] samples = new float[AudioMeter.SampleCount];
    private readonly Complex[] spectrum = new Complex[AudioMeter.SampleCount];
    private readonly float[] levels = new float[27];
    private readonly float[] window = Enumerable.Range(0, AudioMeter.SampleCount)
        .Select(i => (float)(0.5 - 0.5 * Math.Cos(2 * Math.PI * i / (AudioMeter.SampleCount - 1)))).ToArray();
    private readonly PointF[] waveform = new PointF[128];
    private int revision = -1;
    private bool hasFrame;
    private bool fixture;

    [DesignerSerializationVisibility(DesignerSerializationVisibility.Hidden)]
    internal string Mode { get; set; } = "spectrum";
    [DesignerSerializationVisibility(DesignerSerializationVisibility.Hidden)]
    internal bool Smooth { get; set; } = true;
    [DesignerSerializationVisibility(DesignerSerializationVisibility.Hidden)]
    internal Color Accent { get; set; }
    [DesignerSerializationVisibility(DesignerSerializationVisibility.Hidden)]
    internal Color Grid { get; set; }

    public void Read()
    {
        if (meter.Revision == revision) return;
        revision = meter.CopyTo(samples);
        UpdateFrame();
    }

    public void Clear()
    {
        if (!hasFrame) return;
        hasFrame = false;
        Array.Clear(levels);
        Invalidate();
    }

    public void SetFixture()
    {
        fixture = true;
        for (var i = 0; i < samples.Length; i++)
        {
            var t = i / 48000.0;
            samples[i] = (float)(Math.Sin(2 * Math.PI * 127 * t) * 0.28 +
                                 Math.Sin(2 * Math.PI * 341 * t) * 0.14 + Math.Sin(2 * Math.PI * 1107 * t) * 0.08);
        }
        UpdateFrame();
    }

    private void UpdateFrame()
    {
        hasFrame = true;
        if (Mode == "spectrum")
        {
            for (var i = 0; i < samples.Length; i++) spectrum[i] = new(samples[i] * window[i], 0);
            Transform(spectrum);
            for (var bar = 0; bar < levels.Length; bar++)
            {
                var low = Math.Max(1, (int)(Math.Pow(1.255, bar) * 2));
                var high = Math.Clamp((int)(Math.Pow(1.255, bar + 1) * 2), low + 1, spectrum.Length / 2);
                var maximum = 0d;
                for (var bin = low; bin < high; bin++)
                    maximum = Math.Max(maximum, spectrum[bin].Magnitude);
                var db = 20 * Math.Log10(Math.Max(maximum * 4 / spectrum.Length, 0.00001));
                var target = (float)Math.Clamp((db + 62) / 62, 0, 1);
                if (fixture && target < 0.08f)
                    target = (float)(0.10 + 0.22 * (1 + Math.Sin(bar * 2.5)) / 2 * Math.Exp(-bar / 21.0));
                levels[bar] = Smooth && !fixture ? Math.Max(target, levels[bar] * 0.83f) : target;
            }
        }
        Invalidate();
    }

    protected override void OnPaint(PaintEventArgs e)
    {
        base.OnPaint(e);
        var bounds = ClientRectangle;
        if (bounds.Width <= 0 || bounds.Height <= 0 || Mode == "off") return;
        e.Graphics.SmoothingMode = SmoothingMode.AntiAlias;
        using var pen = new Pen(Grid);
        e.Graphics.DrawLine(pen, 0, bounds.Height / 2, bounds.Width, bounds.Height / 2);
        if (!hasFrame) return;
        if (Mode == "waveform")
        {
            using var trace = new Pen(Accent, 1.6f);
            for (var i = 0; i < waveform.Length; i++)
                waveform[i] = new(i * bounds.Width / (float)(waveform.Length - 1),
                    bounds.Height * (0.5f - samples[i * samples.Length / waveform.Length] * 0.43f));
            e.Graphics.DrawLines(trace, waveform);
            return;
        }
        using var brush = new SolidBrush(Accent);
        var step = bounds.Width / (float)levels.Length;
        for (var i = 0; i < levels.Length; i++)
        {
            var height = Math.Max(2, levels[i] * (bounds.Height - 8));
            e.Graphics.FillRectangle(brush, i * step + 2, bounds.Height - height - 3, Math.Max(1, step - 5), height);
        }
    }

    private static void Transform(Complex[] values)
    {
        for (int i = 1, j = 0; i < values.Length; i++)
        {
            var bit = values.Length >> 1;
            for (; (j & bit) != 0; bit >>= 1) j ^= bit;
            j ^= bit;
            if (i < j) (values[i], values[j]) = (values[j], values[i]);
        }
        for (var size = 2; size <= values.Length; size <<= 1)
        {
            var root = Complex.FromPolarCoordinates(1, -2 * Math.PI / size);
            for (var start = 0; start < values.Length; start += size)
            {
                var factor = Complex.One;
                for (var offset = 0; offset < size / 2; offset++)
                {
                    var left = start + offset;
                    var right = left + size / 2;
                    var odd = factor * values[right];
                    values[right] = values[left] - odd;
                    values[left] += odd;
                    factor *= root;
                }
            }
        }
    }
}
