using System.Runtime.InteropServices;
using Windows.Media;

namespace DesktopRadio;

internal sealed class MediaControls : IDisposable
{
    private readonly SystemMediaTransportControls controls;
    private readonly Action<SystemMediaTransportControlsButton> handleButton;

    private MediaControls(SystemMediaTransportControls controls, Action<SystemMediaTransportControlsButton> handleButton)
    {
        this.controls = controls;
        this.handleButton = handleButton;
        controls.IsPlayEnabled = true;
        controls.IsPauseEnabled = true;
        controls.IsStopEnabled = true;
        controls.IsNextEnabled = true;
        controls.IsPreviousEnabled = true;
        controls.ButtonPressed += ButtonPressed;
    }

    public static MediaControls? Attach(nint window, Action<SystemMediaTransportControlsButton> handleButton)
    {
        try { return new(SystemMediaTransportControlsInterop.GetForWindow(window), handleButton); }
        catch (COMException) { return null; }
    }

    public void Update(Station? station, Playback playback)
    {
        try
        {
            controls.IsEnabled = station is not null;
            if (station is null) return;
            controls.PlaybackStatus = playback.State switch
            {
                PlayState.Connecting => MediaPlaybackStatus.Changing,
                PlayState.Playing => MediaPlaybackStatus.Playing,
                _ => MediaPlaybackStatus.Stopped
            };
            var display = controls.DisplayUpdater;
            display.Type = MediaPlaybackType.Music;
            display.AppMediaId = station.Id;
            display.MusicProperties.Title = playback.Track ?? station.Name;
            display.MusicProperties.Artist = playback.Track is null ? "" : station.Name;
            display.Update();
        }
        catch (COMException) { }
    }

    private void ButtonPressed(SystemMediaTransportControls sender, SystemMediaTransportControlsButtonPressedEventArgs args) =>
        handleButton(args.Button);

    public void Dispose()
    {
        try
        {
            controls.ButtonPressed -= ButtonPressed;
            controls.IsEnabled = false;
        }
        catch (COMException) { }
    }
}
