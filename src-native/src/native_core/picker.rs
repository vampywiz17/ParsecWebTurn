//! Computer discovery and list updates without window/control APIs.
use super::account::{self, Host, Session};
use anyhow::{ensure, Result};
use std::sync::{mpsc, Arc};

pub const AUTO_REFRESH_MS: u32 = 30_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refresh {
    Manual,
    Background,
}
impl Refresh {
    pub fn quiet(self) -> bool {
        self == Self::Background
    }
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn poll_ms(self) -> u32 {
        if self.quiet() {
            500
        } else {
            80
        }
    }
}

pub enum Completion {
    Hosts(Refresh, Result<Vec<Host>>),
    Disconnected,
}
struct Request {
    mode: Refresh,
    incoming: mpsc::Receiver<Result<Vec<Host>>>,
}

/// A changed list needs one view update; an identical reply needs none.
pub struct ListChange {
    pub selected: Option<usize>,
    // List scrolling exists only in the Windows list box.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub top: usize,
    pub peer: Option<String>,
}
pub struct ListPosition {
    pub selected: Option<usize>,
    pub last_peer: String,
    pub top: usize,
}

#[derive(Default)]
pub struct Picker {
    hosts: Vec<Host>,
    request: Option<Request>,
}
impl Picker {
    pub fn hosts(&self) -> &[Host] {
        &self.hosts
    }
    pub fn peer_at(&self, index: Option<usize>) -> Option<&str> {
        self.hosts.get(index?).map(|host| host.peer.as_str())
    }
    pub fn blocks_controls(&self) -> bool {
        self.request.as_ref().is_some_and(|r| !r.mode.quiet())
    }
    fn begin(&mut self, mode: Refresh) -> Option<mpsc::Sender<Result<Vec<Host>>>> {
        if self.request.is_some() {
            return None;
        }
        let (tx, incoming) = mpsc::channel();
        self.request = Some(Request { mode, incoming });
        Some(tx)
    }
    pub fn start(&mut self, session: Arc<Session>, mode: Refresh) -> bool {
        let Some(tx) = self.begin(mode) else {
            return false;
        };
        // Same bounded HTTP call and per-request worker as the native launcher.
        std::thread::spawn(move || {
            let _ = tx.send(account::hosts(&session));
        });
        true
    }
    pub fn poll(&mut self) -> Option<Completion> {
        let request = self.request.as_ref()?;
        let completion = match request.incoming.try_recv() {
            Ok(result) => Completion::Hosts(request.mode, result),
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => Completion::Disconnected,
        };
        self.request = None;
        Some(completion)
    }
    pub fn cancel(&mut self) {
        // Drop the receiver: an old HTTP reply cannot update a new login/view.
        // The existing HTTP timeout still bounds the worker's lifetime.
        self.request = None;
    }
    pub fn replace_hosts(
        &mut self,
        hosts: Vec<Host>,
        mode: Refresh,
        position: impl FnOnce() -> ListPosition,
    ) -> Option<ListChange> {
        if self.hosts == hosts {
            return None;
        }
        let position = position();
        let selected_peer = self
            .peer_at(position.selected)
            .unwrap_or(&position.last_peer);
        let selected = (!hosts.is_empty()).then(|| {
            hosts
                .iter()
                .position(|h| h.peer == selected_peer)
                .unwrap_or(0)
        });
        let peer = selected
            .filter(|_| !mode.quiet())
            .map(|i| hosts[i].peer.clone());
        let top = position.top.min(hosts.len().saturating_sub(1));
        self.hosts = hosts;
        Some(ListChange {
            selected,
            top,
            peer,
        })
    }
}

pub fn connection_peer(input: &str) -> Result<String> {
    let peer = input.trim();
    ensure!(
        !peer.is_empty() && peer.len() <= 256 && !peer.chars().any(char::is_control),
        "Select a computer or enter a valid host peer ID."
    );
    Ok(peer.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn host(peer: &str, name: &str) -> Host {
        Host {
            peer: peer.into(),
            name: name.into(),
        }
    }
    fn position(selected: Option<usize>, last_peer: &str, top: usize) -> ListPosition {
        ListPosition {
            selected,
            last_peer: last_peer.into(),
            top,
        }
    }

    #[test]
    fn refresh_is_single_flight_and_cancel_discards_old_replies() {
        let mut picker = Picker::default();
        let old = picker.begin(Refresh::Background).unwrap();
        assert!(!picker.blocks_controls());
        assert!(picker.poll().is_none());
        assert!(picker.begin(Refresh::Manual).is_none());
        picker.cancel();
        let new = picker.begin(Refresh::Manual).unwrap();
        assert!(picker.blocks_controls());
        assert!(old.send(Ok(vec![host("old", "Stale")])).is_err());
        new.send(Ok(vec![host("new", "Current")])).unwrap();
        let Some(Completion::Hosts(mode, Ok(hosts))) = picker.poll() else {
            panic!("missing reply")
        };
        assert_eq!(mode, Refresh::Manual);
        assert_eq!(hosts, vec![host("new", "Current")]);
        assert!(!picker.blocks_controls());
        assert!(picker.poll().is_none());
        let failed = picker.begin(Refresh::Background).unwrap();
        failed.send(Err(account::ExpiredSession.into())).unwrap();
        let Some(Completion::Hosts(Refresh::Background, Err(error))) = picker.poll() else {
            panic!("missing discovery failure")
        };
        assert!(error.downcast_ref::<account::ExpiredSession>().is_some());
        drop(picker.begin(Refresh::Background).unwrap());
        assert!(matches!(picker.poll(), Some(Completion::Disconnected)));
        assert!(picker.begin(Refresh::Manual).is_some());
    }

    #[test]
    fn changed_lists_preserve_identity_and_scroll_without_overwriting_manual_entry() {
        let mut picker = Picker::default();
        let initial = vec![host("a", "First"), host("b", "Second")];
        let change = picker
            .replace_hosts(initial.clone(), Refresh::Manual, || position(None, "b", 4))
            .unwrap();
        assert_eq!(
            (change.selected, change.top, change.peer.as_deref()),
            (Some(1), 1, Some("b"))
        );
        assert!(picker
            .replace_hosts(initial, Refresh::Background, || panic!(
                "unchanged list must not query view state"
            ))
            .is_none());
        let change = picker
            .replace_hosts(
                vec![host("b", "Renamed"), host("a", "First")],
                Refresh::Background,
                || position(Some(1), "a", 1),
            )
            .unwrap();
        assert_eq!(
            (change.selected, change.top, change.peer),
            (Some(0), 1, None)
        );
        assert_eq!(picker.peer_at(change.selected), Some("b"));
        let change = picker
            .replace_hosts(vec![host("a", "First")], Refresh::Manual, || {
                position(Some(0), "b", 9)
            })
            .unwrap();
        assert_eq!(
            (change.selected, change.top, change.peer.as_deref()),
            (Some(0), 0, Some("a"))
        );
        let change = picker
            .replace_hosts(vec![], Refresh::Manual, || position(Some(0), "a", 0))
            .unwrap();
        assert_eq!((change.selected, change.top, change.peer), (None, 0, None));
    }

    #[test]
    fn manual_peer_validation_keeps_the_existing_limits() {
        assert_eq!(connection_peer("  host-id  ").unwrap(), "host-id");
        for invalid in [String::new(), " \t ".into(), "a\nb".into(), "a".repeat(257)] {
            assert!(connection_peer(&invalid).is_err());
        }
        assert!(connection_peer(&"a".repeat(256)).is_ok());
    }
}
