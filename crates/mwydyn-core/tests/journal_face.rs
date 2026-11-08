//! §8 face-level conformance — the InMemory provider. The SQLite
//! provider (feature `sqlite`) must satisfy THESE SAME TESTS; that
//! is the substrate-independence proof.

use mwydyn_core::journal::{InMemoryJournal, JournalFace, JournalOp, Pending};
use mwydyn_core::{ClockFace, ManualClock};
use mwydyn_data::{Address, Letter};

fn letter(payload: &str) -> Letter {
    Letter::new("test.j@1", payload.as_bytes().to_vec(), None, None, vec![]).unwrap()
}

fn pending(to_actor: &str, payload: &str) -> Pending {
    Pending {
        letter: letter(payload),
        from: Address::Instance("external".into()),
        to_actor: to_actor.into(),
        to: Address::Instance(to_actor.into()),
        producer: "test".into(),
    }
}

#[test]
fn appends_stamp_identity_and_replay() {
    let mut j = InMemoryJournal::new(Box::new(ManualClock(1000)));
    let c = j
        .commit(
            "ctx",
            vec![
                JournalOp::Append(pending("a", "one")),
                JournalOp::Append(pending("a", "two")),
            ],
        )
        .unwrap();
    assert_eq!((c.lineage, c.first_seq, c.count), (1, 1, 2));
    assert_eq!(c.commit_ts, 1000);
    let envs = j.read("ctx", 1, 1, 2).unwrap();
    assert_eq!(envs.len(), 2);
    assert_eq!(envs[0].seq, 1);
    assert_eq!(envs[1].seq, 2);
    assert_eq!(envs[0].commit_ts, Some(1000));
    assert!(!envs[0].checksum().is_empty());
}

#[test]
fn pending_and_cursor_flow() {
    let mut j = InMemoryJournal::new(Box::new(ManualClock(0)));
    j.commit(
        "ctx",
        vec![
            JournalOp::Append(pending("a", "m1")),
            JournalOp::Append(pending("b", "m2")),
            JournalOp::Append(pending("a", "m3")),
        ],
    )
    .unwrap();
    assert_eq!(j.pending_count("ctx", "a").unwrap(), 2);
    let p = j.pending("ctx", "a").unwrap();
    assert_eq!((p[0].seq, p[1].seq), (1, 3));
    assert_eq!(j.pending_count("ctx", "b").unwrap(), 1);

    j.commit(
        "ctx",
        vec![JournalOp::Ack {
            actor: "a".into(),
            position: 1,
        }],
    )
    .unwrap();
    assert_eq!(j.pending_count("ctx", "a").unwrap(), 1);
    assert_eq!(j.cursor("ctx", 1, "a").unwrap(), Some(1));
    assert_eq!(j.pending("ctx", "a").unwrap()[0].seq, 3);
}

#[test]
fn fork_creates_ghost_branch_with_shared_prefix() {
    let mut j = InMemoryJournal::new(Box::new(ManualClock(0)));
    j.commit(
        "ctx",
        vec![
            JournalOp::Append(pending("a", "p1")),
            JournalOp::Append(pending("a", "p2")),
            JournalOp::Append(pending("a", "p3")),
        ],
    )
    .unwrap();
    // Fork at 2: the future beyond p2 is an orphaned branch.
    let c = j
        .commit("ctx", vec![JournalOp::Fork { parent_seq: 2 }])
        .unwrap();
    assert_eq!(c.lineage, 2);

    // New appends continue the seq space THROUGH the shared prefix —
    // no copying, delegation on read.
    j.commit("ctx", vec![JournalOp::Append(pending("a", "new1"))])
        .unwrap();
    let l2 = j.read("ctx", 2, 1, 3).unwrap();
    assert_eq!(l2.len(), 3);
    assert_eq!(l2[0].letter.payload, b"p1");
    assert_eq!(l2[1].letter.payload, b"p2");
    assert_eq!(l2[2].letter.payload, b"new1");

    // THE GHOST READ: lineage 1's tail beyond the fork still exists.
    let ghosts = j.read("ctx", 1, 3, 3).unwrap();
    assert_eq!(ghosts.len(), 1);
    assert_eq!(ghosts[0].letter.payload, b"p3");

    // Beyond-history errors loudly, never a silent gap.
    assert!(j.read("ctx", 2, 4, 4).is_err());
    assert!(j.read("ctx", 1, 4, 4).is_err());

    let lins = j.lineages("ctx").unwrap();
    assert_eq!(lins.len(), 2);
    assert!(!lins[0].active);
    assert!(lins[1].active);
    assert_eq!(lins[1].parent, Some((1, 2)));
}

#[test]
fn fork_carries_unacked_prefix_letters_and_clamps_cursors() {
    let mut j = InMemoryJournal::new(Box::new(ManualClock(0)));
    j.commit(
        "ctx",
        vec![
            JournalOp::Append(pending("a", "m1")),
            JournalOp::Append(pending("a", "m2")),
            JournalOp::Append(pending("a", "ghost")),
        ],
    )
    .unwrap();
    // Actor delivers m1 only (cursor 1).
    j.commit(
        "ctx",
        vec![JournalOp::Ack {
            actor: "a".into(),
            position: 1,
        }],
    )
    .unwrap();
    // Rewind to 2: m2 is still pending (within the shared prefix);
    // ghost is beyond it and stops being deliverable.
    j.commit("ctx", vec![JournalOp::Fork { parent_seq: 2 }])
        .unwrap();
    assert_eq!(j.pending("ctx", "a").unwrap().len(), 1);
    assert_eq!(j.pending("ctx", "a").unwrap()[0].letter.payload, b"m2");
    assert_eq!(j.cursor("ctx", 2, "a").unwrap(), Some(1));
}
