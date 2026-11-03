use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const VERSION: u32 = 2;
pub const MAX_PACKET: usize = 262_144;
pub const LEASE_MS: u64 = 1000;
const TOMBSTONE_MS: u64 = 60_000;
const CAPACITY: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: String,
    pub deadline: u64,
    pub pidl: Vec<u8>,
    pub path: String,
    pub selection: Vec<String>,
    pub flags: i32,
    pub new_tab: bool,
    pub new_window: bool,
}

impl Request {
    pub fn valid(&self, now: u64) -> bool {
        let path = |value: &str| {
            !value.is_empty()
                && !value.contains(['\0', '"'])
                && value.encode_utf16().count() <= 32767
                && std::path::Path::new(value).is_absolute()
        };
        self.id.len() == 32
            && self.id.bytes().all(|value| value.is_ascii_hexdigit())
            && self.deadline > now
            && self.deadline <= now.saturating_add(LEASE_MS)
            && path(&self.path)
            && self.selection.len() <= 32
            && self.selection.iter().all(|value| path(value))
            && !(self.new_tab && self.new_window)
            && matches!(self.flags, 0 | 4 | 8)
            && valid_pidl(&self.pidl)
    }
}

pub fn valid_pidl(bytes: &[u8]) -> bool {
    if bytes.len() < 2 || bytes.len() > 65_536 {
        return false;
    }
    let mut offset = 0;
    while offset + 2 <= bytes.len() {
        let length = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]) as usize;
        if length == 0 {
            return offset + 2 == bytes.len();
        }
        if length < 2 || offset + length > bytes.len() {
            return false;
        }
        offset += length;
    }
    false
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", content = "payload", deny_unknown_fields)]
pub enum Operation {
    Prepare(Request),
    Commit(String),
    Cancel(String),
    Query(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Message {
    pub version: u32,
    pub operation: Operation,
}

impl Message {
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() > MAX_PACKET {
            return None;
        }
        serde_json::from_slice::<Self>(bytes)
            .ok()
            .filter(|value| value.version == VERSION)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    Unknown,
    Prepared,
    Queued,
    Executing,
    Applying,
    Completed,
    Cancelled,
    Expired,
    Rejected,
    Conflict,
    Ambiguous,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub version: u32,
    pub id: String,
    pub state: State,
}

struct Entry {
    request: Request,
    state: State,
    retained_until: u64,
}

#[derive(Default)]
pub struct Ledger {
    entries: BTreeMap<String, Entry>,
}

impl Ledger {
    pub fn receive(&mut self, message: Message, now: u64) -> Reply {
        let id = match &message.operation {
            Operation::Prepare(request) => request.id.clone(),
            Operation::Commit(id) | Operation::Cancel(id) | Operation::Query(id) => id.clone(),
        };
        if message.version != VERSION || id.len() != 32 {
            return Reply {
                version: VERSION,
                id,
                state: State::Rejected,
            };
        }
        if let Operation::Prepare(request) = &message.operation {
            self.entries
                .retain(|_, entry| entry.retained_until > now || entry.state == State::Applying);
            if let Some(entry) = self.entries.get(&id) {
                if entry.request != *request {
                    return Reply {
                        version: VERSION,
                        id,
                        state: State::Conflict,
                    };
                }
            } else {
                if !request.valid(now) || self.entries.len() >= CAPACITY {
                    return Reply {
                        version: VERSION,
                        id,
                        state: State::Rejected,
                    };
                }
                self.entries.insert(
                    id.clone(),
                    Entry {
                        request: request.clone(),
                        state: State::Prepared,
                        retained_until: request.deadline.saturating_add(TOMBSTONE_MS),
                    },
                );
            }
        }
        let Some(entry) = self.entries.get_mut(&id) else {
            return Reply {
                version: VERSION,
                id,
                state: State::Unknown,
            };
        };
        if now >= entry.request.deadline
            && matches!(
                entry.state,
                State::Prepared | State::Queued | State::Executing
            )
        {
            entry.state = State::Expired;
        }
        match message.operation {
            Operation::Commit(_) if entry.state == State::Prepared => entry.state = State::Queued,
            Operation::Cancel(_)
                if matches!(
                    entry.state,
                    State::Prepared | State::Queued | State::Executing
                ) =>
            {
                entry.state = State::Cancelled;
            }
            _ => {}
        }
        Reply {
            version: VERSION,
            id,
            state: entry.state,
        }
    }

    pub fn begin(&mut self, id: &str, now: u64) -> Option<Request> {
        let entry = self.entries.get_mut(id)?;
        if entry.state != State::Queued {
            return None;
        }
        if now >= entry.request.deadline {
            entry.state = State::Expired;
            return None;
        }
        entry.state = State::Executing;
        Some(entry.request.clone())
    }

    pub fn finish(&mut self, id: &str, painted: bool) {
        if let Some(entry) = self.entries.get_mut(id) {
            if entry.state == State::Applying {
                entry.state = if painted {
                    State::Completed
                } else {
                    State::Ambiguous
                };
            }
        }
    }

    pub fn apply(&mut self, id: &str, now: u64) -> bool {
        let Some(entry) = self.entries.get_mut(id) else {
            return false;
        };
        if entry.state != State::Executing {
            return false;
        }
        if now >= entry.request.deadline {
            entry.state = State::Expired;
            return false;
        }
        entry.state = State::Applying;
        true
    }

    pub fn cancel_unpresented(&mut self, id: &str) {
        if let Some(entry) = self.entries.get_mut(id) {
            if entry.state != State::Completed {
                entry.state = State::Cancelled;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> Request {
        Request {
            id: "0123456789abcdef0123456789abcdef".into(),
            deadline: 1600,
            path: r"C:\fixture".into(),
            pidl: vec![0, 0],
            selection: Vec::new(),
            flags: 0,
            new_tab: true,
            new_window: false,
        }
    }

    fn send(ledger: &mut Ledger, operation: Operation, now: u64) -> State {
        ledger
            .receive(
                Message {
                    version: VERSION,
                    operation,
                },
                now,
            )
            .state
    }

    #[test]
    fn cancellation_is_terminal_and_late_commit_cannot_open() {
        for before_commit in [true, false] {
            let mut ledger = Ledger::default();
            let request = request();
            let id = request.id.clone();
            assert_eq!(
                send(&mut ledger, Operation::Prepare(request.clone()), 1000),
                State::Prepared
            );
            if !before_commit {
                send(&mut ledger, Operation::Commit(id.clone()), 1100);
            }
            assert_eq!(
                send(&mut ledger, Operation::Cancel(id.clone()), 1200),
                State::Cancelled
            );
            assert_eq!(
                send(&mut ledger, Operation::Prepare(request), 1300),
                State::Cancelled
            );
            assert_eq!(
                send(&mut ledger, Operation::Commit(id.clone()), 1300),
                State::Cancelled
            );
            assert!(ledger.begin(&id, 1400).is_none());
        }
    }

    #[test]
    fn deadline_covers_ui_queue_and_prevents_delayed_execution() {
        let mut ledger = Ledger::default();
        let request = request();
        let id = request.id.clone();
        send(&mut ledger, Operation::Prepare(request), 1000);
        send(&mut ledger, Operation::Commit(id.clone()), 1100);
        assert!(ledger.begin(&id, 1600).is_none());
        assert_eq!(
            send(&mut ledger, Operation::Query(id), 1700),
            State::Expired
        );
    }

    #[test]
    fn duplicate_and_lost_ack_reconcile_without_reexecution() {
        let mut ledger = Ledger::default();
        let request = request();
        let id = request.id.clone();
        send(&mut ledger, Operation::Prepare(request.clone()), 1000);
        send(&mut ledger, Operation::Commit(id.clone()), 1100);
        assert!(ledger.begin(&id, 1200).is_some());
        assert!(ledger.begin(&id, 1200).is_none());
        assert!(ledger.apply(&id, 1200));
        assert_eq!(
            send(&mut ledger, Operation::Cancel(id.clone()), 1300),
            State::Applying
        );
        ledger.finish(&id, true);
        assert_eq!(
            send(&mut ledger, Operation::Prepare(request), 1800),
            State::Completed
        );
        assert_eq!(
            send(&mut ledger, Operation::Commit(id), 1800),
            State::Completed
        );
    }

    #[test]
    fn id_collision_and_unversioned_or_oversized_messages_rejected() {
        let mut ledger = Ledger::default();
        let mut request = request();
        send(&mut ledger, Operation::Prepare(request.clone()), 1000);
        request.path = r"C:\other".into();
        assert_eq!(
            send(&mut ledger, Operation::Prepare(request), 1000),
            State::Conflict
        );
        assert!(Message::decode(br#"{"version":3,"operation":"Query","payload":"x"}"#).is_none());
        assert!(Message::decode(&vec![b' '; MAX_PACKET + 1]).is_none());
    }

    #[test]
    fn malformed_pidls_and_unsafe_paths_are_rejected() {
        for bytes in [
            vec![],
            vec![0],
            vec![1, 0, 0, 0],
            vec![0, 0, 1],
            vec![6, 0, 1, 0],
        ] {
            assert!(!valid_pidl(&bytes));
        }
        assert!(valid_pidl(&[4, 0, 1, 2, 0, 0]));
        for path in ["relative", "C:\\nul\0bad", "C:\\quote\"bad"] {
            let mut r = request();
            r.path = path.into();
            assert!(!r.valid(1000));
        }
    }

    #[test]
    fn bootstrap_lease_is_accepted_and_cannot_extend_the_protocol_limit() {
        let mut r = request();
        r.deadline = 1000 + LEASE_MS;
        assert!(r.valid(1000));
        r.deadline += 1;
        assert!(!r.valid(1000));
        r.deadline = 1000;
        assert!(!r.valid(1000));
    }

    #[test]
    fn post_commit_failure_is_ambiguous_and_never_authorizes_replay() {
        let mut ledger = Ledger::default();
        let request = request();
        let id = request.id.clone();
        send(&mut ledger, Operation::Prepare(request), 1000);
        send(&mut ledger, Operation::Commit(id.clone()), 1100);
        ledger.begin(&id, 1200).unwrap();
        ledger.apply(&id, 1200);
        ledger.finish(&id, false);
        assert_eq!(
            send(&mut ledger, Operation::Cancel(id), 2000),
            State::Ambiguous
        );
    }

    #[test]
    fn tombstones_are_bounded_without_evicting_live_decisions() {
        let mut ledger = Ledger::default();
        for index in 0..CAPACITY {
            let mut r = request();
            r.id = format!("{index:032x}");
            assert_eq!(
                send(&mut ledger, Operation::Prepare(r), 1000),
                State::Prepared
            );
        }
        assert_eq!(
            send(&mut ledger, Operation::Prepare(request()), 1000),
            State::Rejected
        );
        let mut r = request();
        r.deadline = 100_600;
        assert_eq!(
            send(&mut ledger, Operation::Prepare(r), 100_000),
            State::Prepared
        );
    }
    #[test]
    fn stalled_execution_cannot_apply_after_cancel_or_lease_expiry() {
        for cancel in [false, true] {
            let mut ledger = Ledger::default();
            let request = request();
            let id = request.id.clone();
            send(&mut ledger, Operation::Prepare(request), 1000);
            send(&mut ledger, Operation::Commit(id.clone()), 1100);
            ledger.begin(&id, 1200).unwrap();
            if cancel {
                assert_eq!(
                    send(&mut ledger, Operation::Cancel(id.clone()), 1250),
                    State::Cancelled
                );
            }
            assert!(!ledger.apply(&id, 1700));
            assert!(matches!(
                send(&mut ledger, Operation::Query(id), 1800),
                State::Cancelled | State::Expired
            ));
        }
    }

    #[test]
    fn cancelled_unpresented_navigation_cannot_complete_or_restart() {
        let mut ledger = Ledger::default();
        let request = request();
        let id = request.id.clone();
        send(&mut ledger, Operation::Prepare(request), 1000);
        send(&mut ledger, Operation::Commit(id.clone()), 1100);
        ledger.begin(&id, 1200).unwrap();
        assert!(ledger.apply(&id, 1200));
        ledger.cancel_unpresented(&id);
        ledger.finish(&id, true);
        assert_eq!(
            send(&mut ledger, Operation::Query(id.clone()), 1300),
            State::Cancelled
        );
        assert!(ledger.begin(&id, 1300).is_none());
    }
}
