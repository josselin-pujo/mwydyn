use mwydyn_vectors::{check_corpus, load_corpus};

fn run(file: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../vectors")
        .join(file);
    let corpus = load_corpus(&path).unwrap_or_else(|e| panic!("{} loads: {}", file, e));
    let failures = check_corpus(&corpus.cases);
    if !failures.is_empty() {
        panic!(
            "{}: {} vector failures:\n{}",
            file,
            failures.len(),
            failures.join("\n")
        );
    }
}

#[test]
fn cbor_profile_v0() {
    run("cbor-profile-v0.json");
}

#[test]
fn blob_refs_v0() {
    run("blob-refs-v0.json");
}
