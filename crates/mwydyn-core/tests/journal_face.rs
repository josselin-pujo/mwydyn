//! §8 face-level conformance — run against EVERY provider. The
//! in-memory and SQLite implementations must be indistinguishable
//! through the face; that is the substrate-independence proof (§8.5)
//! and the reason the double implementation cost is worth paying.

use mwydyn_core::journal::{InMemoryJournal, JournalFace, JournalOp, Pending};
use mwydyn_core::{ClockFace, ManualClock};
use mwydyn_data::{Address, Letter};

#[cfg(feature = "sqlite")]
use mwydyn_core::SQLiteJournal;
#[cfg(feature = "sqlite")]
use std::path::PathBuf;

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

fn providers() -> Vec<(&'static str, Box<dyn JournalFace>)> {
    let mut v: Vec<(&'static str, Box<dyn JournalFace>)> = vec![(
        "inmem",
        Box::new(InMemoryJournal::new(Box::new(ManualClock(1000)))),
    )];
    #[cfg(feature = "sqlite")]
    v.push((
        "sqlite-mem",
        Box::new(SQLiteJournal::open_memory(Box::new(ManualClock(1000))).unwrap()),
    ));
    v
}

#[test]
fn appends_stamp_identity_and_replay() {
    for (name, mut j) in providers() {
        let c = j
            .commit(
                "ctx",
                vec![
                    JournalOp::Append(pending("a", "one")),
                    JournalOp::Append(pending("a", "two")),
                ],
            )
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!((c.lineage, c.first_seq, c.count), (1, 1, 2), "{name}");
        assert_eq!(c.commit_ts, 1000, "{name}");
        let envs = j.read("ctx", 1, 1, 2).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(envs.len(), 2, "{name}");
        assert_eq!(envs[0].seq, 1, "{name}");
        assert_eq!(envs[1].seq, 2, "{name}");
        assert_eq!(envs[0].commit_ts, Some(1000), "{name}");
        assert!(!envs[0].checksum().is_empty(), "{name}");
    }
}

#[test]
fn pending_and_cursor_flow() {
    for (name, mut j) in providers() {
        j.commit(
            "ctx",
            vec![
                JournalOp::Append(pending("a", "m1")),
                JournalOp::Append(pending("b", "m2")),
                JournalOp::Append(pending("a", "m3")),
            ],
        )
        .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(j.pending_count("ctx", "a").unwrap(), 2, "{name}");
        let p = j.pending("ctx", "a").unwrap();
        assert_eq!((p[0].seq, p[1].seq), (1, 3), "{name}");
        assert_eq!(j.pending_count("ctx", "b").unwrap(), 1, "{name}");

        j.commit(
            "ctx",
            vec![JournalOp::Ack {
                actor: "a".into(),
                position: 1,
            }],
        )
        .unwrap();
        assert_eq!(j.pending_count("ctx", "a").unwrap(), 1, "{name}");
        assert_eq!(j.cursor("ctx", 1, "a").unwrap(), Some(1), "{name}");
        assert_eq!(j.pending("ctx", "a").unwrap()[0].seq, 3, "{name}");
    }
}

#[test]
fn fork_creates_ghost_branch_with_shared_prefix() {
    for (name, mut j) in providers() {
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
        assert_eq!(c.lineage, 2, "{name}");

        // New appends continue the seq space THROUGH the shared
        // prefix — no copying, delegation on read.
        j.commit("ctx", vec![JournalOp::Append(pending("a", "new1"))])
            .unwrap();
        let l2 = j.read("ctx", 2, 1, 3).unwrap();
        assert_eq!(l2.len(), 3, "{name}");
        assert_eq!(l2[0].letter.payload, b"p1", "{name}");
        assert_eq!(l2[1].letter.payload, b"p2", "{name}");
        assert_eq!(l2[2].letter.payload, b"new1", "{name}");

        // THE GHOST READ: lineage 1's tail beyond the fork still
        // exists, readable, never deliverable.
        let ghosts = j.read("ctx", 1, 3, 3).unwrap();
        assert_eq!(ghosts.len(), 1, "{name}");
        assert_eq!(ghosts[0].letter.payload, b"p3", "{name}");

        // Beyond-history errors loudly, never a silent gap.
        assert!(j.read("ctx", 2, 4, 4).is_err(), "{name}");
        assert!(j.read("ctx", 1, 4, 4).is_err(), "{name}");

        let lins = j.lineages("ctx").unwrap();
        assert_eq!(lins.len(), 2, "{name}");
        assert!(!lins[0].active, "{name}");
        assert!(lins[1].active, "{name}");
        assert_eq!(lins[1].parent, Some((1, 2)), "{name}");
    }
}

#[test]
fn fork_carries_unacked_prefix_letters_and_clamps_cursors() {
    for (name, mut j) in providers() {
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
        // Rewind to 2: m2 is still pending (within the shared
        // prefix); ghost is beyond it and stops being deliverable.
        j.commit("ctx", vec![JournalOp::Fork { parent_seq: 2 }])
            .unwrap();
        assert_eq!(j.pending("ctx", "a").unwrap().len(), 1, "{name}");
        assert_eq!(
            j.pending("ctx", "a").unwrap()[0].letter.payload,
            b"m2",
            "{name}"
        );
        assert_eq!(j.cursor("ctx", 2, "a").unwrap(), Some(1), "{name}");
    }
}

#[test]
#[cfg(feature = "sqlite")]
fn sqlite_persists_across_reopen() {
    // The file-format proof: drop the process-shaped handle, reopen,
    // and the journal is exactly what it was — entries, cursors,
    // pending, and lineages (§8.4: no retention loss).
    let dir = std::env::temp_dir();
    let path: PathBuf =
        dir.join(format!("mwydyn-jtest-{}-{:?}.sqlite", std::process::id(), line!()));
    let _ = std::fs::remove_file(&path);
    {
        let mut j = SQLiteJournal::open(&path, Box::new(ManualClock(1000))).unwrap();
        j.commit(
            "ctx",
            vec![
                JournalOp::Append(pending("a", "m1")),
                JournalOp::Append(pending("a", "m2")),
                JournalOp::Ack {
                    actor: "a".into(),
                    position: 1,
                },
                JournalOp::Fork { parent_seq: 1 },
                JournalOp::Append(pending("a", "m3")),
            ],
        )
        .unwrap();
    }
    {
        let j = SQLiteJournal::open(&path, Box::new(ManualClock(1000))).unwrap();
        // Lineage 2 = shared prefix (m1) + m3 at seq 2 (the fork was
        // at parent seq 1 — the seq space continues THROUGH it).
        let l2 = j.read("ctx", 2, 1, 2).unwrap();
        assert_eq!(l2.len(), 2);
        assert_eq!(l2[0].letter.payload, b"m1");
        assert_eq!(l2[1].letter.payload, b"m3");
        // The ghost branch survived too: lineage 1's tail beyond the
        // fork (m2 at seq 2) is still there, readable, not delivered.
        let ghosts = j.read("ctx", 1, 2, 2).unwrap();
        assert_eq!(ghosts[0].letter.payload, b"m2");
        // Cursor survived; pending on lineage 2 = m3 only.
        assert_eq!(j.cursor("ctx", 2, "a").unwrap(), Some(1));
        assert_eq!(j.pending("ctx", "a").unwrap().len(), 1);
        assert_eq!(j.pending("ctx", "a").unwrap()[0].letter.payload, b"m3");
        assert!(j.active_lineage("ctx").unwrap() >= 2);
    }
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
}
