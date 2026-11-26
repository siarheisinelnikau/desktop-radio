use crate::{audio::Status, model::Station};
use windows::{
    Foundation::TypedEventHandler,
    Media::{
        MediaPlaybackStatus, MediaPlaybackType, SystemMediaTransportControls,
        SystemMediaTransportControlsButton,
    },
    Win32::{
        Foundation::HWND,
        System::WinRT::{
            ISystemMediaTransportControlsInterop, RO_INIT_SINGLETHREADED, RoGetActivationFactory,
            RoInitialize, RoUninitialize,
        },
    },
    core::HSTRING,
};

pub struct MediaSession {
    controls: SystemMediaTransportControls,
    token: i64,
}

#[derive(Clone, Copy)]
pub enum MediaKey {
    Play,
    Pause,
    Next,
    Previous,
}

impl MediaSession {
    pub fn attach(window: HWND, notify: impl Fn(MediaKey) + Send + 'static) -> Option<Self> {
        if unsafe { RoInitialize(RO_INIT_SINGLETHREADED) }.is_err() {
            return None;
        }
        let result = Self::open(window, notify);
        if result.is_err() {
            unsafe {
                RoUninitialize();
            }
        }
        result.ok()
    }

    fn open(
        window: HWND,
        notify: impl Fn(MediaKey) + Send + 'static,
    ) -> windows::core::Result<Self> {
        let factory: ISystemMediaTransportControlsInterop = unsafe {
            RoGetActivationFactory(&HSTRING::from("Windows.Media.SystemMediaTransportControls"))?
        };
        let controls: SystemMediaTransportControls = unsafe { factory.GetForWindow(window)? };
        controls.SetIsPlayEnabled(true)?;
        controls.SetIsPauseEnabled(true)?;
        controls.SetIsStopEnabled(true)?;
        controls.SetIsNextEnabled(true)?;
        controls.SetIsPreviousEnabled(true)?;
        let handler = TypedEventHandler::new(
            move |_,
                  arguments: windows::core::Ref<
                '_,
                windows::Media::SystemMediaTransportControlsButtonPressedEventArgs,
            >| {
                let Some(args) = arguments.as_ref() else {
                    return Ok(());
                };
                if let Some(action) = match args.Button()? {
                    SystemMediaTransportControlsButton::Play => Some(MediaKey::Play),
                    SystemMediaTransportControlsButton::Pause
                    | SystemMediaTransportControlsButton::Stop => Some(MediaKey::Pause),
                    SystemMediaTransportControlsButton::Next => Some(MediaKey::Next),
                    SystemMediaTransportControlsButton::Previous => Some(MediaKey::Previous),
                    _ => None,
                } {
                    notify(action);
                }
                Ok(())
            },
        );
        let token = controls.ButtonPressed(&handler)?;
        Ok(Self { controls, token })
    }

    pub fn update(&self, station: Option<&Station>, status: &Status) -> windows::core::Result<()> {
        self.controls.SetIsEnabled(station.is_some())?;
        let Some(station) = station else {
            return Ok(());
        };
        self.controls.SetPlaybackStatus(match status {
            Status::Playing(_) => MediaPlaybackStatus::Playing,
            Status::Connecting => MediaPlaybackStatus::Changing,
            _ => MediaPlaybackStatus::Stopped,
        })?;
        let display = self.controls.DisplayUpdater()?;
        display.SetType(MediaPlaybackType::Music)?;
        display.SetAppMediaId(&HSTRING::from(&station.id))?;
        let music = display.MusicProperties()?;
        let (title, artist) = if let Status::Playing(Some(track)) = status {
            (track.as_str(), station.name.as_str())
        } else {
            (station.name.as_str(), "")
        };
        music.SetTitle(&HSTRING::from(title))?;
        music.SetArtist(&HSTRING::from(artist))?;
        display.Update()
    }
}

impl Drop for MediaSession {
    fn drop(&mut self) {
        let _ = self.controls.RemoveButtonPressed(self.token);
        let _ = self.controls.SetIsEnabled(false);
        unsafe {
            RoUninitialize();
        }
    }
}
