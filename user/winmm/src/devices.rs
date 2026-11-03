//! Waveform audio and MIDI devices: gala has none. The counts are 0, and
//! every call naming a device id fails with MMSYSERR_BADDEVICEID, every
//! call on a handle (none can have been opened) with MMSYSERR_INVALHANDLE,
//! as Windows answers on a machine without sound hardware. Chromium's
//! media code imports these (and asks for the counts at start-up).

const MMSYSERR_BADDEVICEID: u32 = 2;
const MMSYSERR_INVALHANDLE: u32 = 5;

macro_rules! no_devices {
    ($($name:ident),*) => {$(
        #[unsafe(no_mangle)]
        pub extern "system" fn $name() -> u32 {
            0
        }
    )*};
}

macro_rules! bad_device {
    ($($name:ident),*) => {$(
        #[unsafe(no_mangle)]
        pub extern "system" fn $name() -> u32 {
            MMSYSERR_BADDEVICEID
        }
    )*};
}

macro_rules! bad_handle {
    ($($name:ident),*) => {$(
        #[unsafe(no_mangle)]
        pub extern "system" fn $name() -> u32 {
            MMSYSERR_INVALHANDLE
        }
    )*};
}

no_devices!(waveInGetNumDevs, waveOutGetNumDevs, midiInGetNumDevs, midiOutGetNumDevs);

// (uDeviceID, ...): no device has an id.
bad_device!(
    waveOutOpen,
    waveInOpen,
    midiInOpen,
    midiOutOpen,
    midiInGetDevCapsW,
    midiOutGetDevCapsW
);

// (handle, ...): no handle was ever opened.
bad_handle!(
    waveOutClose,
    waveOutPause,
    waveOutPrepareHeader,
    waveOutReset,
    waveOutRestart,
    waveOutUnprepareHeader,
    waveOutWrite,
    midiInAddBuffer,
    midiInClose,
    midiInPrepareHeader,
    midiInReset,
    midiInStart,
    midiInUnprepareHeader,
    midiOutClose,
    midiOutLongMsg,
    midiOutPrepareHeader,
    midiOutReset,
    midiOutShortMsg,
    midiOutUnprepareHeader
);
