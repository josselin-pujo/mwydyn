//! overlay-v0 — THE OVERLAY FACE CONFORMANCE (round 39). The
//! round-34 lesson, made permanent: the durability overlay drains
//! through the FACE (`Box<dyn JournalFace>`), and a provider that
//! records via inherent methods only is a SILENT NO-OP through the
//! trait object — exactly the bug that shipped in round 31 and hid
//! until the round-34 verification. These vectors drive the three
//! roles through the face and pin the contract:
//!
//! 1. the RECORDING skin records commits and drains exactly;
//! 2. a DURABILITY face is silent (it never reports unsynced ops);
//! 3. a plain HOT tier without the recording skin is silent too —
//!    recording is the overlay's job, not the provider's default.
//!
//! Any future provider (NATS/Kafka hot views, quorum logs) satisfies
//! the same three vectors.

use mwydyn_core::journal::{
    InMemoryJournal, JournalFace, JournalOp, Pending, RecordingJournal,
};
use mwydyn_core::{ManualClock, SQLiteJournal};

fn ops(n: usize) -> Vec<JournalOp> {
    (0..n)
        .map(|i| {
            JournalOp::Append(Pending {
                letter: mwydyn_data::Letter::new(
                    "test.op@1",
                    mwydyn_data::encode(&mwydyn_data::Value::Int(i as i64)),
                    None,
                    None,
                    vec![],
                )
                .unwrap(),
                from: mwydyn_data::Address::Instance("external".into()),
                to_actor: "sink".into(),
                to: mwydyn_data::Address::Named {
                    context: "c".into(),
                    name: "sink".into(),
                },
                ack: None,
                producer: "external".into(),
            })
        })
        .collect()
}

fn fresh_path(tag: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!(
        "mwydyn-overlay-v0-{}-{}-{:?}.sqlite",
        tag,
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_file(&p);
    p
}

#[test]
fn overlay_v0_recording_skin_records_and_drains_exactly() {
    // through the FACE — the trait object is where the round-34
    // no-op hid
    let mut face: Box<dyn JournalFace> = Box::new(RecordingJournal::new(Box::new(
        InMemoryJournal::new(Box::new(ManualClock(0))),
    )));
    face.commit("c", ops(3)).expect("commit");
    assert_eq!(
        face.unsynced_ops().len(),
        3,
        "the recording skin reports every committed op through the face"
    );
    let drained = face.take_ops(2);
    assert_eq!(drained.len(), 2, "take_ops returns exactly n");
    assert_eq!(
        face.unsynced_ops().len(),
        1,
        "the remainder stays queued for the next drain"
    );
}

#[test]
fn overlay_v0_durability_face_is_silent() {
    let path = fresh_path("durable");
    let mut face: Box<dyn JournalFace> = Box::new(
        SQLiteJournal::open(&path, Box::new(ManualClock(0))).expect("sqlite opens"),
    );
    face.commit("c", ops(2)).expect("commit");
    assert_eq!(
        face.unsynced_ops().len(),
        0,
        "a DURABILITY face never reports unsynced ops — it IS the durability point"
    );
    assert!(face.take_ops(5).is_empty());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn overlay_v0_plain_hot_tier_is_silent() {
    // InMemoryJournal WITHOUT the recording skin: recording is the
    // OVERLAY's job — tiers without a durable face pay nothing.
    let mut face: Box<dyn JournalFace> =
        Box::new(InMemoryJournal::new(Box::new(ManualClock(0))));
    face.commit("c", ops(1)).expect("commit");
    assert_eq!(
        face.unsynced_ops().len(),
        0,
        "a plain hot tier records nothing (the overlay wrapper records)"
    );
    assert!(face.take_ops(5).is_empty());
}
