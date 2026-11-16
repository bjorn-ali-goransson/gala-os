//! Where the install of a catalog package is, for its start menu tile.
//!
//! A tile whose program is not installed runs `gala install <id>`
//! (ROADMAP SM) without a window: gala says where the install is through
//! GalaDesktopAction (`nt::action`'s ACTION_INSTALL_PROGRESS), the tile
//! draws it as a bar, and a failure leaves the bar red for a click to
//! try again, as a Linux distribution's install does (`desktop::linux`).

/// Where an install is, as the tile shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    /// Reading the source's manifests: the package, its version, its
    /// installer.
    Resolving,
    /// Downloading the installer, this much of it (percent).
    Downloading(u8),
    /// Writing it to the machine: an installer run, or a portable
    /// package unpacked.
    Installing,
    /// It failed. The bar stays, in red; a click tries again. Why is in
    /// the log (`nt::action`: gala's exit code), not on the tile.
    Failed,
}

impl Progress {
    /// What `gala install` reports: `state` 0 resolving, 1 downloading
    /// (`percent`), 2 installing, 4 failed. None for a state no tile
    /// draws (3, done: the install's progress goes away) or one this
    /// kernel does not know.
    pub fn from_state(state: i32, percent: i32) -> Option<Progress> {
        match state {
            0 => Some(Progress::Resolving),
            1 => Some(Progress::Downloading(percent.clamp(0, 100) as u8)),
            2 => Some(Progress::Installing),
            4 => Some(Progress::Failed),
            _ => None,
        }
    }

    /// How much of the bar is filled. The download is its middle three
    /// fifths, since that is where an install spends its time and the
    /// only part whose own progress is known; resolving shows a sliver
    /// and the writing nearly all of it.
    pub fn percent(self) -> u8 {
        match self {
            Progress::Resolving => 5,
            Progress::Downloading(p) => (10 + u32::from(p.min(100)) * 3 / 5) as u8,
            Progress::Installing => 85,
            Progress::Failed => 100,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_states_gala_reports_fill_the_bar_in_order() {
        assert_eq!(Progress::from_state(0, 0), Some(Progress::Resolving));
        assert_eq!(Progress::from_state(1, 50), Some(Progress::Downloading(50)));
        assert_eq!(Progress::from_state(2, 0), Some(Progress::Installing));
        assert_eq!(Progress::from_state(4, 0), Some(Progress::Failed));
        // Done takes the bar away, and a state from a newer gala.exe is
        // not drawn rather than drawn wrongly.
        assert_eq!(Progress::from_state(3, 0), None);
        assert_eq!(Progress::from_state(9, 0), None);
        // A percentage outside 0..100 (a host that counted wrongly) is
        // clamped, not wrapped.
        assert_eq!(Progress::from_state(1, -5), Some(Progress::Downloading(0)));
        assert_eq!(Progress::from_state(1, 1000), Some(Progress::Downloading(100)));
        // The bar never goes backwards as an install goes on.
        let steps = [
            Progress::Resolving,
            Progress::Downloading(0),
            Progress::Downloading(50),
            Progress::Downloading(100),
            Progress::Installing,
        ];
        assert!(steps.windows(2).all(|w| w[0].percent() < w[1].percent()), "{steps:?}");
        assert_eq!(Progress::Downloading(100).percent(), 70);
        assert_eq!(Progress::Failed.percent(), 100);
    }
}
