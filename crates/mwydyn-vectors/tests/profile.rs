use mwydyn_vectors::{check_corpus, load_corpus};

#[test]
fn cbor_profile_v0() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../vectors/cbor-profile-v0.json");
    let corpus = load_corpus(&path).expect("corpus loads");
    let failures = check_corpus(&corpus.cases);
    if !failures.is_empty() {
        panic!(
            "{} vector failures:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}
