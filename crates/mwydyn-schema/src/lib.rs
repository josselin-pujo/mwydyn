//! mwydyn-schema — the schema registry (spec §5, §7.9).
//!
//! Ids are `<namespace>.<name>@<major>`; **majors are immutable once
//! ratified** (a bug becomes a new major, never an edit); within a
//! major, revisions are additive-only. Definitions are carried in
//! BOTH representations the profile mandates (round 10): the CDDL
//! text (authoritative, verbatim) and the canonical CBOR projection
//! (storage + orthogonality) — the registry digest pins the
//! projection, so every consumer agrees byte-for-byte on what a
//! schema IS. Claims are idempotent for identical digests and refuse
//! conflicts loudly at composition.
//!
//! v0 scope note (work-notes/17): the registry carries the CDDL
//! verbatim and pins digests; payload VALIDATION against CDDL is M1+
//! (needs a parser). The core stays schema-blind either way (§7.9).

use mwydyn_data::{sha256_hex, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaDef {
    /// `<namespace>.<name>@<major>` (spec §7.9).
    pub id: String,
    /// The authoritative definition, carried verbatim (round 10:
    /// CDDL authoritative; a parser turns it into validators — M1+).
    pub cddl: String,
    /// Optional human summary — metadata, never semantics.
    pub summary: Option<String>,
}

impl SchemaDef {
    pub fn new(id: impl Into<String>, cddl: impl Into<String>) -> SchemaDef {
        SchemaDef {
            id: id.into(),
            cddl: cddl.into(),
            summary: None,
        }
    }

    pub fn with_summary(mut self, summary: impl Into<String>) -> SchemaDef {
        self.summary = Some(summary.into());
        self
    }

    /// The canonical CBOR projection the digest pins (round 10: the
    /// registry practices the data model's own projection law).
    pub fn to_value(&self) -> Value {
        Value::map(vec![
            ("id".into(), Value::Text(self.id.clone())),
            (
                "summary".into(),
                match &self.summary {
                    Some(s) => Value::Text(s.clone()),
                    None => Value::Null,
                },
            ),
            ("cddl".into(), Value::Text(self.cddl.clone())),
        ])
        .expect("fixed hygienic keys")
    }

    /// SHA-256 over the canonical CBOR projection — what two
    /// implementations compare to agree they mean the same schema.
    pub fn digest(&self) -> String {
        sha256_hex(&mwydyn_data::encode(&self.to_value()))
    }

    pub fn valid(&self) -> Result<(), SchemaError> {
        if !mwydyn_data::envelope::valid_kind(&self.id) {
            return Err(SchemaError::BadId(self.id.clone()));
        }
        if self.cddl.trim().is_empty() {
            return Err(SchemaError::EmptyCddl);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaError {
    BadId(String),
    EmptyCddl,
    /// Same id, different definition — refuse at composition (§7.9:
    /// conflicting definitions for one id refuse; the namespace is
    /// claimed per context).
    Conflict {
        id: String,
        pinned: String,
        incoming: String,
    },
}

#[derive(Debug, Default, Clone)]
pub struct Registry {
    defs: Vec<(SchemaDef, String)>, // (def, pinned digest), insertion order
}

impl Registry {
    pub fn new() -> Registry {
        Registry { defs: Vec::new() }
    }

    pub fn with(defs: impl IntoIterator<Item = SchemaDef>) -> Registry {
        let mut r = Registry::new();
        for d in defs {
            r.claim(d).expect("core schemas never conflict");
        }
        r
    }

    /// Claim a namespace entry. Idempotent for identical digests
    /// (composition is commutative); conflicts refuse loudly (§7.9).
    pub fn claim(&mut self, def: SchemaDef) -> Result<String, SchemaError> {
        def.valid()?;
        let digest = def.digest();
        if let Some((_, pinned)) = self.lookup_digest(&def.id) {
            if *pinned == digest {
                return Ok(digest); // idempotent claim
            }
            return Err(SchemaError::Conflict {
                id: def.id,
                pinned: pinned.clone(),
                incoming: digest,
            });
        }
        self.defs.push((def, digest.clone()));
        Ok(digest)
    }

    /// Compose another registry into this one (embeddings compose
    /// theirs over the runtime's core schemas).
    pub fn compose(&mut self, other: &Registry) -> Result<(), SchemaError> {
        for (def, _) in &other.defs {
            self.claim(def.clone())?;
        }
        Ok(())
    }

    pub fn lookup(&self, id: &str) -> Option<&SchemaDef> {
        self.defs.iter().find(|(d, _)| d.id == id).map(|(d, _)| d)
    }

    pub fn lookup_digest(&self, id: &str) -> Option<(&SchemaDef, &String)> {
        self.defs.iter().find(|(d, _)| d.id == id).map(|(d, g)| (d, g))
    }

    pub fn len(&self) -> usize {
        self.defs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&SchemaDef, &String)> {
        self.defs.iter().map(|(d, g)| (d, g))
    }
}

/// The runtime's core schemas (§7.9: mwydyn ships mwydyn.* — every
/// embedding composes these; contexts seed their registry with them).
pub fn core_schemas() -> Vec<SchemaDef> {
    vec![
        SchemaDef::new(
            "mwydyn.timer.arm@1",
            "; Arm a one-shot timer. DURATIONS, never timestamps (§6):\n\
             ; the ticker anchors `afterUs` at the arm letter's commit.\n\
             arm = { afterUs: uint }\n",
        )
        .with_summary("arm a one-shot timer: tick to the armer after afterUs"),
        SchemaDef::new(
            "mwydyn.timer.tick@1",
            "; The tick: an EMPTY payload — the timer's identity rides\n\
             ; the envelope correlation (echoed from the arm).\n\
             tick = {}\n",
        )
        .with_summary("fired tick; identity = envelope correlation"),
        SchemaDef::new(
            "mwydyn.timer.poll@1",
            "; Pump-injected while timers are armed (timers fire as\n\
             ; messages, never as callbacks). Not application surface.\n\
             poll = {}\n",
        )
        .with_summary("runtime pump poke — internal"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claims_digests_and_idempotence() {
        let mut r = Registry::new();
        let a = SchemaDef::new("acme.thing@1", "thing = { x: int }");
        let d1 = r.claim(a.clone()).unwrap();
        let d2 = r.claim(a).unwrap();
        assert_eq!(d1, d2, "identical claims are idempotent");
        assert_eq!(d1.len(), 64);

        let conflicting = SchemaDef::new("acme.thing@1", "thing = { y: int }");
        match r.claim(conflicting) {
            Err(SchemaError::Conflict { id, pinned, incoming }) => {
                assert_eq!(id, "acme.thing@1");
                assert_ne!(pinned, incoming);
            }
            other => panic!("expected a conflict, got {:?}", other.is_ok()),
        }

        // Compose is idempotent for identical definitions.
        let mut r2 = Registry::with(core_schemas());
        r2.compose(&r).unwrap();
        assert_eq!(r2.lookup("acme.thing@1").unwrap().cddl, "thing = { x: int }");
    }

    #[test]
    fn core_schemas_are_stable_across_registries() {
        let a = Registry::with(core_schemas());
        let b = Registry::with(core_schemas());
        for (def, digest) in a.iter() {
            assert_eq!(b.lookup_digest(&def.id).unwrap().1, digest);
        }
        for id in ["mwydyn.timer.arm@1", "mwydyn.timer.tick@1", "mwydyn.timer.poll@1"] {
            assert!(a.lookup(id).is_some(), "{id} ships with the runtime");
        }
    }
}
