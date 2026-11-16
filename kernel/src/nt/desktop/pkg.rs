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
    /// It failed, with gala's exit code (`install_failure` says what it
    /// stands for). The bar stays, in red; a click tries again. Why is
    /// in the log and in the tile's notification, not on the tile.
    Failed(i32),
    /// Its `gala install` is gone (or never started) without saying how
    /// it went, so no word is coming: this far it had got (percent of
    /// the bar). The bar stays there, in grey under a question mark; a
    /// click tries again. gala never reports this - the desktop
    /// concludes it when the installer's process ends
    /// (`Desktop::installers_gone`).
    Lost(u8),
}

impl Progress {
    /// What `gala install` reports: `state` 0 resolving, 1 downloading
    /// (`percent`), 2 installing, 4 failed (`exit`: what gala ended
    /// with). None for a state no tile draws (3, done: the install's
    /// progress goes away) or one this kernel does not know. `Lost` is
    /// not among them: it is what the desktop makes of an installer
    /// that said nothing at all.
    pub fn from_state(state: i32, percent: i32, exit: i32) -> Option<Progress> {
        match state {
            0 => Some(Progress::Resolving),
            1 => Some(Progress::Downloading(percent.clamp(0, 100) as u8)),
            2 => Some(Progress::Installing),
            4 => Some(Progress::Failed(exit)),
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
            Progress::Failed(_) => 100,
            Progress::Lost(at) => at.min(100),
        }
    }

    /// The install is over, one way or another: its bar stays where it
    /// is and a click tries again.
    pub fn ended(self) -> bool {
        matches!(self, Progress::Failed(_) | Progress::Lost(_))
    }
}

/// What a `gala install` exit code says beyond the install having
/// failed (galapkg's `Kind`), as a clause: the log says `failed (gala
/// exit N: <this>)` and the tile's notification `The install failed:
/// <this>.` None when the code says no more than that, so that neither
/// says it twice over.
pub fn install_failure(exit: i32) -> Option<&'static str> {
    match exit {
        1 => Some("gala was asked for it wrongly"),
        2 => Some("the source has no such package or version"),
        3 => Some("gala cannot handle that installer or source yet"),
        5 => Some("the download is not what the manifest says"),
        6 => Some("it needs administrator rights"),
        // 4 is galapkg's `Failed`: a step of the install did not go
        // through and gala's own line says which, in the log where a
        // windowless install's output goes. Any other code is a gala
        // this kernel does not know.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An install whose installer is gone keeps its bar where it got
    /// to, and is over as a failure is.
    #[test]
    fn an_install_gone_quiet_keeps_the_bar_where_it_got_to() {
        let at = Progress::Downloading(50).percent();
        assert_eq!(Progress::Lost(at).percent(), at);
        assert!(Progress::Lost(at).ended() && Progress::Failed(4).ended());
        assert!(!Progress::Resolving.ended() && !Progress::Installing.ended());
        assert!(!Progress::Downloading(50).ended());
        // gala reports no such state: the desktop concludes it.
        for state in 0..10 {
            assert!(!matches!(Progress::from_state(state, 0, 0), Some(Progress::Lost(_))));
        }
    }

    /// Each code galapkg exits with says something of its own; the one
    /// that says only that it failed (4), and a code from a newer gala,
    /// say nothing more rather than guess.
    #[test]
    fn every_exit_code_says_what_it_knows() {
        let said: alloc::vec::Vec<Option<&str>> = (1..=6).map(install_failure).collect();
        for (i, s) in said.iter().enumerate() {
            assert!(s.is_none_or(|s| !s.is_empty()));
            assert!(s.is_none() || !said[..i].contains(s), "exit {} says what another does", i + 1);
        }
        assert_eq!(install_failure(4), None);
        assert_eq!(install_failure(7), None);
        assert_eq!(install_failure(-1), None);
    }

    #[test]
    fn the_states_gala_reports_fill_the_bar_in_order() {
        assert_eq!(Progress::from_state(0, 0, 0), Some(Progress::Resolving));
        assert_eq!(Progress::from_state(1, 50, 0), Some(Progress::Downloading(50)));
        assert_eq!(Progress::from_state(2, 0, 0), Some(Progress::Installing));
        // A failure keeps the code gala ended with, for the tile's
        // notification and the log.
        assert_eq!(Progress::from_state(4, 0, 4), Some(Progress::Failed(4)));
        // Done takes the bar away, and a state from a newer gala.exe is
        // not drawn rather than drawn wrongly.
        assert_eq!(Progress::from_state(3, 0, 0), None);
        assert_eq!(Progress::from_state(9, 0, 0), None);
        // A percentage outside 0..100 (a host that counted wrongly) is
        // clamped, not wrapped.
        assert_eq!(Progress::from_state(1, -5, 0), Some(Progress::Downloading(0)));
        assert_eq!(Progress::from_state(1, 1000, 0), Some(Progress::Downloading(100)));
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
        assert_eq!(Progress::Failed(4).percent(), 100);
    }
}
